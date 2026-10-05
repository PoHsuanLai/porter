//! The Microsoft sign-in conversation: PKCE through a loopback redirect, or a device code, then
//! one Graph read to learn the address and what the tenant allows, then a review.
//!
//! The host feeds `Start`, then `Poll` until the browser (or the other device) has answered,
//! then `Confirm`. `Cancel` ends it at any step and frees the listeners.

use super::env::{MicrosoftEnv, SignInFlow};
use super::graph::{Found, probe};
use super::launcher::Launcher;
use super::scopes::{GRAPH_DEFAULT, graph_only, scopes_for};
use super::{declared_kinds, graph_origin, imap_origin};
use porter_core::capability::{Capability, CapabilityKind, Offered};
use porter_core::sheet::{SignInFault, SignInInput, UserCode};
use porter_core::{
    AccountLabel, Credential, EndpointUrl, Family, LoginName, Offer, Restriction, SecretPurpose,
    ServiceEndpoint, TenantConsent, Tls, UnixSeconds,
};
use porter_http::Http;
use porter_oauth::{
    AuthCode, DeviceCodeResponse, DeviceFault, DevicePoll, ExchangeFault, LoopbackFault,
    LoopbackServer, OAuthState, Pkce, TokenResponse, authorize_url, endpoints_of,
    exchange_code_scoped, poll_device, redeem_scope, refresh_scoped, request_device_code,
};
use porter_provider::{
    ClientEntry, Issuer, IssuerEndpoints, ProviderSpec, SignIn, SignInStart, SignInStep, Signed,
};
use tokio::task::JoinHandle;

/// Seconds a device flow is slowed by a `slow_down` answer (RFC 8628 section 3.5).
const SLOW_DOWN_SECONDS: i64 = 5;
/// Exchange Online's SMTP submission server, which the file has no row for.
const SMTP_ORIGIN: &str = "smtp://smtp.office365.com:587";

/// The Microsoft sign-in.
pub struct MicrosoftSignIn<H = porter_http::HyperHttp> {
    spec: ProviderSpec,
    env: MicrosoftEnv<H>,
    start: SignInStart,
    phase: Phase,
}

impl<H> std::fmt::Debug for MicrosoftSignIn<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MicrosoftSignIn")
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
    /// The person is typing a code on another device.
    Device(Box<Device>),
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
struct Grant {
    client: ClientEntry,
    endpoints: IssuerEndpoints,
    scopes: Vec<String>,
}

#[derive(Debug)]
struct Browser {
    grant: Grant,
    pkce: Pkce,
    redirect: String,
    wait: Task<Result<AuthCode, LoopbackFault>>,
    _launcher: Launcher,
}

#[derive(Debug)]
struct Device {
    grant: Grant,
    code: DeviceCodeResponse,
    interval: i64,
    due: UnixSeconds,
    expires: UnixSeconds,
}

impl<H> MicrosoftSignIn<H> {
    pub(super) fn new(spec: ProviderSpec, env: MicrosoftEnv<H>, start: SignInStart) -> Self {
        Self {
            spec,
            env,
            start,
            phase: Phase::Fresh,
        }
    }
}

impl<H: Http + 'static> SignIn for MicrosoftSignIn<H> {
    async fn next(&mut self, input: SignInInput) -> SignInStep {
        let phase = std::mem::replace(&mut self.phase, Phase::Over);
        if input == SignInInput::Cancel {
            return SignInStep::Failed(SignInFault::Cancelled);
        }
        let step = match phase {
            Phase::Fresh => self.begin().await,
            Phase::Browser(browser) => self.await_browser(*browser).await,
            Phase::Device(device) => self.await_device(*device).await,
            Phase::Review(signed) => self.review_or_done(*signed, input),
            Phase::Over => Err(SignInFault::Cancelled),
        };
        step.unwrap_or_else(SignInStep::Failed)
    }
}

type Step = Result<SignInStep, SignInFault>;

impl<H: Http + 'static> MicrosoftSignIn<H> {
    fn waiting(&mut self, phase: Phase) -> Step {
        self.phase = phase;
        Ok(SignInStep::Waiting)
    }

    async fn begin(&mut self) -> Step {
        let client = self
            .env
            .registry
            .lookup(Issuer::Microsoft, self.env.channel)
            .cloned()
            .ok_or(SignInFault::NeedsClientId)?;
        let grant = Grant {
            endpoints: endpoints_of(&client),
            scopes: scopes_for(&declared_kinds(&self.spec)),
            client,
        };
        match self.env.flow {
            SignInFlow::Loopback => self.begin_loopback(grant).await,
            SignInFlow::DeviceCode => self.begin_device(grant).await,
        }
    }

    async fn begin_loopback(&mut self, grant: Grant) -> Step {
        let (verifier, state) = (self.env.random)().ok_or(SignInFault::Unreadable)?;
        let pkce = Pkce::from_random(verifier, state);
        let server = LoopbackServer::bind()
            .await
            .map_err(|_| SignInFault::Unreachable)?;
        let redirect = server.redirect_uri();
        let target = authorize_url(
            &grant.endpoints,
            &grant.client,
            &pkce,
            &redirect,
            &grant.scopes,
        );
        let launcher = Launcher::start(target)
            .await
            .map_err(|_| SignInFault::Unreachable)?;
        let url = EndpointUrl::parse(&launcher.page()).map_err(|_| SignInFault::Unreadable)?;
        let expected: OAuthState = pkce.state.clone();
        let wait = Task(tokio::spawn(async move { server.wait(&expected).await }));
        self.phase = Phase::Browser(Box::new(Browser {
            grant,
            pkce,
            redirect,
            wait,
            _launcher: launcher,
        }));
        Ok(SignInStep::OpenBrowser { url })
    }

    async fn begin_device(&mut self, grant: Grant) -> Step {
        let code = request_device_code(
            &*self.env.http,
            &grant.endpoints,
            &grant.client,
            &grant.scopes.join(" "),
        )
        .await
        .map_err(device_fault)?;
        let url =
            EndpointUrl::parse(&code.verification_uri).map_err(|_| SignInFault::Unreadable)?;
        let now = (self.env.clock)();
        let step = SignInStep::ShowCode {
            user_code: UserCode(code.user_code.clone()),
            url,
        };
        self.phase = Phase::Device(Box::new(Device {
            interval: i64::from(code.interval.max(1)),
            due: UnixSeconds(now.0 + i64::from(code.interval.max(1))),
            expires: UnixSeconds(now.0 + i64::from(code.expires_in)),
            grant,
            code,
        }));
        Ok(step)
    }

    async fn await_browser(&mut self, mut browser: Browser) -> Step {
        let arrived = tokio::time::timeout(self.env.poll_slice, &mut browser.wait.0).await;
        let code = match arrived {
            Err(_elapsed) => return self.waiting(Phase::Browser(Box::new(browser))),
            Ok(Err(_panicked)) => return Err(SignInFault::Unreadable),
            Ok(Ok(outcome)) => outcome.map_err(loopback_fault)?,
        };
        let Browser {
            grant,
            pkce,
            redirect,
            ..
        } = browser;
        let tokens = exchange_code_scoped(
            &*self.env.http,
            &grant.endpoints,
            &grant.client,
            &pkce,
            &code,
            &redirect,
            redeem_scope(&grant.scopes).as_deref(),
        )
        .await
        .map_err(exchange_fault)?;
        self.conclude(grant, tokens).await
    }

    async fn await_device(&mut self, mut device: Device) -> Step {
        tokio::time::sleep(self.env.poll_slice).await;
        let now = (self.env.clock)();
        if now >= device.expires {
            return Err(SignInFault::TimedOut);
        }
        if now < device.due {
            return self.waiting(Phase::Device(Box::new(device)));
        }
        let polled = poll_device(
            &*self.env.http,
            &device.grant.endpoints,
            &device.grant.client,
            &device.code,
        )
        .await;
        device.due = UnixSeconds(now.0 + device.interval);
        match polled {
            Ok(DevicePoll::Pending) | Err(ExchangeFault::Unreachable) => {
                self.waiting(Phase::Device(Box::new(device)))
            }
            Ok(DevicePoll::SlowDown) => {
                device.interval += SLOW_DOWN_SECONDS;
                device.due = UnixSeconds(now.0 + device.interval);
                self.waiting(Phase::Device(Box::new(device)))
            }
            Ok(DevicePoll::Approved(tokens)) => self.conclude(device.grant, tokens).await,
            Ok(DevicePoll::Denied) => Err(SignInFault::Cancelled),
            Ok(DevicePoll::Expired) => Err(SignInFault::TimedOut),
            Err(fault) => Err(exchange_fault(fault)),
        }
    }

    /// The tokens are in: read the account, and offer the review.
    async fn conclude(&mut self, grant: Grant, tokens: TokenResponse) -> Step {
        let now = (self.env.clock)();
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let (graph, refresh) = match graph_only(&grant.scopes) {
            true => (tokens, refresh),
            false => {
                let graph = refresh_scoped(
                    &*self.env.http,
                    &grant.endpoints,
                    &grant.client,
                    &refresh,
                    Some(GRAPH_DEFAULT),
                )
                .await
                .map_err(exchange_fault)?;
                let rotated = graph.refresh_token.clone().unwrap_or(refresh);
                (graph, rotated)
            }
        };
        let base = graph_origin(&self.spec);
        let found = probe(
            &*self.env.http,
            &self.spec,
            &base,
            graph.access_token.expose(),
        )
        .await
        .map_err(provider_fault)?;
        let signed = signed_from(
            &self.spec,
            found,
            Credential::OAuth {
                expires_at: graph.expires_at(now),
                access: graph.access_token,
                refresh,
            },
        );
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

fn signed_from(spec: &ProviderSpec, found: Found, credential: Credential) -> Signed {
    let endpoints = endpoints_for(spec, &found);
    let restriction = Restriction {
        consent: match found.tenant_refused() {
            true => TenantConsent::AdminRequired,
            false => TenantConsent::User,
        },
        ..Restriction::none()
    };
    Signed {
        label: AccountLabel(found.address),
        credentials: vec![(SecretPurpose::OAuthRefresh, credential)],
        claims: found.claims,
        endpoints,
        restriction,
    }
}

/// The servers of the account: Exchange's IMAP and SMTP when mail is there, Graph when any of
/// its services is.
fn endpoints_for(spec: &ProviderSpec, found: &Found) -> Vec<ServiceEndpoint> {
    let login = LoginName(found.address.clone());
    let present = |kind: CapabilityKind| {
        found.claims.iter().find_map(|c| match &c.offer {
            Offer::Present(cap) if cap.kind() == kind => Some(cap),
            _ => None,
        })
    };
    let endpoint = |family, url: EndpointUrl| {
        let tls = match url.origin().scheme {
            porter_core::UrlScheme::Imap | porter_core::UrlScheme::Smtp => Tls::StartTls,
            porter_core::UrlScheme::Http => Tls::Plain,
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
        out.push(endpoint(Family::Imap, imap_origin(spec)));
        if mail.send == Offered::Present {
            if let Ok(smtp) = EndpointUrl::parse(SMTP_ORIGIN) {
                out.push(endpoint(Family::Smtp, smtp));
            }
        }
    }
    let graph_on = found
        .claims
        .iter()
        .any(|c| matches!(&c.offer, Offer::Present(cap) if cap.kind() != CapabilityKind::Mail));
    if graph_on {
        out.push(endpoint(Family::Graph, graph_origin(spec)));
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

fn device_fault(fault: DeviceFault) -> SignInFault {
    match fault {
        DeviceFault::Unsupported => SignInFault::Unreadable,
        DeviceFault::Denied => SignInFault::Cancelled,
        DeviceFault::Expired => SignInFault::TimedOut,
        DeviceFault::Exchange(inner) => exchange_fault(inner),
    }
}

fn provider_fault(fault: porter_provider::ProviderError) -> SignInFault {
    use porter_provider::ProviderError as E;
    match fault {
        E::Unauthorized => SignInFault::Refused,
        E::Forbidden => SignInFault::Forbidden,
        E::Unreachable => SignInFault::Unreachable,
        E::Unreadable => SignInFault::Unreadable,
    }
}

/// What the issuer's redirect (or the lack of one) says of the sign-in. A person who said no
/// cancelled; a tenant that wants an administrator's approval forbids; the rest were refused.
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
        e if e.contains("consent") || e.contains("admin") => SignInFault::Forbidden,
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
            ("consent_required", SignInFault::Forbidden),
            ("admin_consent_required", SignInFault::Forbidden),
            ("invalid_scope", SignInFault::Refused),
            ("server_error", SignInFault::Refused),
        ];
        for (error, want) in CASES {
            assert_eq!(refusal(error), *want, "{error}");
        }
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
