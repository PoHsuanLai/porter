//! The Google sign-in conversation: PKCE through a loopback redirect, then the address from
//! userinfo and one small read per service to learn what the account allows, then a review.
//!
//! The host feeds `Start`, then `Poll` until the browser has answered, then `Confirm`. `Cancel`
//! ends it at any step and frees the listener. Signing in again (`SignInMode::Reauthenticate`)
//! has no review: it is `Done` after the reads. With no client row for this build the first step
//! is `Failed(NeedsClientId)`: nothing is opened and nothing is dialled.

use super::env::GoogleEnv;
use super::probe::{probe, whoami};
use super::scopes::{Granted, scopes_for};
use super::{Apis, apis_of, declared_kinds};
use porter_core::capability::{Capability, CapabilityKind};
use porter_core::sheet::{SignInFault, SignInInput};
use porter_core::{
    AccountLabel, Claim, Count, Credential, EndpointUrl, Family, Limit, LimitReason, LoginName,
    Offer, Restriction, SecretPurpose, ServiceEndpoint, TenantConsent, Tls, TokenLifetime,
    UrlScheme, Verification, WebUrl,
};
use porter_http::Http;
use porter_oauth::{
    AppReview, AuthCode, ClientTraits, ExchangeFault, LoopbackFault, LoopbackServer, OAuthState,
    Pkce, TokenResponse, authorize_url, endpoints_of, exchange_code,
};
use porter_provider::{
    ClientEntry, Issuer, IssuerEndpoints, ProviderSpec, SignIn, SignInMode, SignInStart,
    SignInStep, Signed,
};
use tokio::task::JoinHandle;

/// Google's cap on the users of an app in testing.
const TESTING_USER_CAP: u32 = 100;
/// Gmail's SMTP submission server, which the file has no row for.
const SMTP_ORIGIN: &str = "smtp://smtp.gmail.com:587";

/// The Google sign-in.
pub struct GoogleSignIn<H = porter_http::HyperHttp> {
    spec: Box<ProviderSpec>,
    env: Box<GoogleEnv<H>>,
    start: SignInStart,
    phase: Phase,
}

impl<H> std::fmt::Debug for GoogleSignIn<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleSignIn")
            .field("start", &self.start)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum Phase {
    /// Nothing sent yet.
    Fresh,
    /// The browser is on its way back to the loopback listener.
    Browser(Box<Browser>),
    /// Everything is found; the person reviews it.
    Review(Box<Signed>),
    /// Done, failed or cancelled.
    Over,
}

/// A task that is aborted when its owner goes away.
#[derive(Debug)]
struct Task<T>(JoinHandle<T>);

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Debug)]
struct Browser {
    client: ClientEntry,
    endpoints: IssuerEndpoints,
    traits: ClientTraits,
    pkce: Pkce,
    redirect: String,
    wait: Task<Result<AuthCode, LoopbackFault>>,
}

impl<H> GoogleSignIn<H> {
    pub(super) fn new(spec: ProviderSpec, env: GoogleEnv<H>, start: SignInStart) -> Self {
        Self {
            spec: Box::new(spec),
            env: Box::new(env),
            start,
            phase: Phase::Fresh,
        }
    }
}

impl<H: Http + 'static> SignIn for GoogleSignIn<H> {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let phase = std::mem::replace(&mut self.phase, Phase::Over);
        if input == SignInInput::Cancel {
            return SignInStep::Failed(SignInFault::Cancelled);
        }
        let step = match phase {
            Phase::Fresh => self.begin().await,
            Phase::Browser(browser) => self.await_browser(*browser).await,
            Phase::Review(signed) => self.review_or_done(*signed, input),
            Phase::Over => Err(SignInFault::Cancelled),
        };
        step.unwrap_or_else(SignInStep::Failed)
    }
}

type Step = Result<SignInStep, SignInFault>;

impl<H: Http + 'static> GoogleSignIn<H> {
    fn waiting(&mut self, phase: Phase) -> Step {
        self.phase = phase;
        Ok(SignInStep::Waiting)
    }

    async fn begin(&mut self) -> Step {
        let client = self
            .env
            .registry
            .lookup(Issuer::Google, self.env.channel)
            .cloned()
            .ok_or(SignInFault::NeedsClientId)?;
        let traits = self.env.registry.traits(Issuer::Google, self.env.channel);
        let scopes = scopes_for(&declared_kinds(&self.spec), traits.mail);
        let (verifier, state) = (self.env.random)().ok_or(SignInFault::Unreadable)?;
        let pkce = Pkce::from_random(verifier, state);
        let server = LoopbackServer::bind()
            .await
            .map_err(|_| SignInFault::Unreachable)?;
        let redirect = server.redirect_uri();
        let endpoints = endpoints_of(&client);
        let target = authorize_url(&endpoints, &client, &pkce, &redirect, &scopes);
        let url = WebUrl::parse(&target).map_err(|_| SignInFault::Unreadable)?;
        let expected: OAuthState = pkce.state.clone();
        let wait = Task(tokio::spawn(async move { server.wait(&expected).await }));
        self.phase = Phase::Browser(Box::new(Browser {
            client,
            endpoints,
            traits,
            pkce,
            redirect,
            wait,
        }));
        Ok(SignInStep::OpenBrowser { url })
    }

    async fn await_browser(&mut self, mut browser: Browser) -> Step {
        let arrived = tokio::time::timeout(self.env.poll_slice, &mut browser.wait.0).await;
        let code = match arrived {
            Err(_elapsed) => return self.waiting(Phase::Browser(Box::new(browser))),
            Ok(Err(_panicked)) => return Err(SignInFault::Unreadable),
            Ok(Ok(outcome)) => outcome.map_err(loopback_fault)?,
        };
        let tokens = exchange_code(
            &*self.env.http,
            &browser.endpoints,
            &browser.client,
            &browser.pkce,
            &code,
            &browser.redirect,
        )
        .await
        .map_err(exchange_fault)?;
        self.conclude(browser.traits, tokens).await
    }

    /// The tokens are in: read the account, and offer the review.
    async fn conclude(&mut self, traits: ClientTraits, tokens: TokenResponse) -> Step {
        let now = (self.env.clock)();
        // `prompt=consent` and `access_type=offline` make Google send one; without it the
        // account would stop working in an hour.
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let granted = Granted::of(tokens.granted_scopes());
        let token = tokens.access_token.expose().to_owned();
        let address = whoami(&*self.env.http, &self.env.userinfo, &token)
            .await
            .map_err(provider_fault)?;
        let claims = probe(
            &*self.env.http,
            &self.spec,
            &address,
            &token,
            (&granted, traits.mail),
        )
        .await
        .map_err(provider_fault)?;
        let apis = apis_of(&self.spec);
        let signed = Signed {
            label: AccountLabel(address.clone()),
            credentials: vec![(
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    expires_at: tokens.expires_at(now),
                    access: tokens.access_token,
                    refresh,
                },
            )],
            endpoints: endpoints_for(&self.spec, &apis, &claims, &address),
            restriction: restriction_for(traits.review, &claims),
            claims,
        };
        // Signing in again does not ask what to use again: the account keeps its services.
        if matches!(self.start.mode, SignInMode::Reauthenticate { .. }) {
            return Ok(SignInStep::Done(signed));
        }
        let step = review_of(&signed);
        self.phase = Phase::Review(Box::new(signed));
        Ok(step)
    }

    fn review_or_done(&mut self, signed: Signed, input: SignInInput) -> Step {
        match input {
            SignInInput::Confirm(_) => Ok(SignInStep::Done(signed)),
            _ => {
                let step = review_of(&signed);
                self.phase = Phase::Review(Box::new(signed));
                Ok(step)
            }
        }
    }
}

fn review_of(signed: &Signed) -> SignInStep {
    SignInStep::Review {
        claims: signed.claims.clone(),
        endpoints: signed.endpoints.clone(),
        restriction: signed.restriction.clone(),
        label: signed.label.clone(),
    }
}

/// What limits the account: a client in testing is unverified (100 users, and a sign-in that
/// lasts seven days), and the Drive and Photos scopes are narrow by design (R3, R4).
fn restriction_for(review: AppReview, claims: &[Claim]) -> Restriction {
    let present = |kind| {
        claims
            .iter()
            .any(|c| matches!(&c.offer, Offer::Present(cap) if cap.kind() == kind))
    };
    let mut limits = Vec::new();
    if present(CapabilityKind::Storage) {
        limits.push(Limit {
            kind: CapabilityKind::Storage,
            reason: LimitReason::AppFolderOnly,
        });
    }
    if present(CapabilityKind::Photos) {
        limits.push(Limit {
            kind: CapabilityKind::Photos,
            reason: LimitReason::PickerOnly,
        });
    }
    let (verification, token_lifetime) = match review {
        AppReview::Testing => (
            Verification::Unverified {
                user_cap: Count(TESTING_USER_CAP),
            },
            TokenLifetime::SevenDays,
        ),
        AppReview::Verified => (Verification::Verified, TokenLifetime::Standard),
    };
    Restriction {
        verification,
        token_lifetime,
        consent: TenantConsent::User,
        limits,
    }
}

/// The servers of the account: Gmail's IMAP and SMTP when mail is there, and the API of every
/// other service that is.
fn endpoints_for(
    spec: &ProviderSpec,
    apis: &Apis,
    claims: &[Claim],
    address: &str,
) -> Vec<ServiceEndpoint> {
    let login = LoginName(address.to_owned());
    let present = |kind: CapabilityKind| {
        claims.iter().find_map(|c| match &c.offer {
            Offer::Present(cap) if cap.kind() == kind => Some(cap),
            _ => None,
        })
    };
    let endpoint = |family, url: EndpointUrl| {
        let tls = match url.origin().scheme {
            UrlScheme::Imap | UrlScheme::Smtp => Tls::StartTls,
            UrlScheme::Http => Tls::Plain,
            _ => Tls::Implicit,
        };
        ServiceEndpoint {
            family,
            url,
            tls,
            login: login.clone(),
        }
    };
    let mut out = Vec::new();
    if let Some(Capability::Mail(mail)) = present(CapabilityKind::Mail) {
        out.push(endpoint(Family::Imap, apis.base(Family::Imap)));
        if let (porter_core::capability::Offered::Present, Ok(smtp)) =
            (mail.send, EndpointUrl::parse(SMTP_ORIGIN))
        {
            out.push(endpoint(Family::Smtp, smtp));
        }
    }
    for row in &spec.capabilities {
        let api = !matches!(row.family, Family::Imap | Family::Smtp);
        if api && present(row.capability.kind()).is_some() && row.endpoint.is_some() {
            out.push(endpoint(row.family, apis.base(row.family)));
        }
    }
    out
}

fn exchange_fault(fault: ExchangeFault) -> SignInFault {
    match fault {
        ExchangeFault::Refused => SignInFault::Refused,
        ExchangeFault::Unreachable => SignInFault::Unreachable,
        ExchangeFault::Unreadable => SignInFault::Unreadable,
    }
}

fn provider_fault(error: porter_provider::ProviderError) -> SignInFault {
    use porter_provider::ProviderError as E;
    match error {
        E::Unauthorized => SignInFault::Refused,
        E::Forbidden => SignInFault::Forbidden,
        E::Unreachable => SignInFault::Unreachable,
        E::Unreadable => SignInFault::Unreadable,
    }
}

/// What the issuer's redirect (or the lack of one) says of the sign-in. A person who said no
/// cancelled; a Workspace that wants an administrator's approval (`admin_policy_enforced`) or a
/// client that is not allowed this person (`access_denied` with a test-user list) forbids; the
/// rest were refused.
fn loopback_fault(fault: LoopbackFault) -> SignInFault {
    match fault {
        LoopbackFault::TimedOut => SignInFault::TimedOut,
        LoopbackFault::Refused(error) => refusal(&error),
        LoopbackFault::WrongState => SignInFault::Refused,
        LoopbackFault::Oversized | LoopbackFault::NoCode | LoopbackFault::Malformed => {
            SignInFault::Unreadable
        }
    }
}

fn refusal(error: &str) -> SignInFault {
    match error {
        "access_denied" => SignInFault::Cancelled,
        e if e.contains("admin") || e.contains("policy") => SignInFault::Forbidden,
        _ => SignInFault::Refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issuers_refusal_is_a_cancel_a_forbid_or_a_refusal() {
        const CASES: &[(&str, SignInFault)] = &[
            ("access_denied", SignInFault::Cancelled),
            ("admin_policy_enforced", SignInFault::Forbidden),
            ("org_internal", SignInFault::Refused),
            ("invalid_scope", SignInFault::Refused),
            ("server_error", SignInFault::Refused),
        ];
        for (error, want) in CASES {
            assert_eq!(refusal(error), *want, "{error}");
        }
    }

    #[test]
    fn a_client_in_testing_is_unverified_for_a_hundred_users_and_seven_days() {
        let testing = restriction_for(AppReview::Testing, &[]);
        assert_eq!(
            testing.verification,
            Verification::Unverified {
                user_cap: Count(100)
            }
        );
        assert_eq!(testing.token_lifetime, TokenLifetime::SevenDays);
        let verified = restriction_for(AppReview::Verified, &[]);
        assert_eq!(verified.verification, Verification::Verified);
        assert_eq!(verified.token_lifetime, TokenLifetime::Standard);
    }

    #[test]
    fn a_redirect_fault_maps_to_what_the_person_sees() {
        const CASES: &[(LoopbackFault, SignInFault)] = &[
            (LoopbackFault::TimedOut, SignInFault::TimedOut),
            (LoopbackFault::WrongState, SignInFault::Refused),
            (LoopbackFault::NoCode, SignInFault::Unreadable),
            (LoopbackFault::Malformed, SignInFault::Unreadable),
            (LoopbackFault::Oversized, SignInFault::Unreadable),
        ];
        for (fault, want) in CASES {
            assert_eq!(loopback_fault(fault.clone()), *want, "{fault:?}");
        }
    }
}
