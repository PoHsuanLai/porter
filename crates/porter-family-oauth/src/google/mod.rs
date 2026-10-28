//! Google's rules (lane W5c): the owner's own Google account through a Google Cloud OAuth client
//! the owner registers (docs/google.md). Sign-in is PKCE through a loopback redirect (the
//! installed-app flow) asking offline access and one set of scopes per capability (the scope
//! catalogue is `issuers.toml`); Calendar, People, Tasks and the Drive app folder are probed at
//! add time, Photos is shown by its scope, and Gmail over IMAP and SMTP with XOAUTH2 is there only
//! for a client that is the person's own (`byo = true` in its row), since its scope is restricted.
//!
//! With no client row the provider is still served: signing in answers `NeedsClientId`, which the
//! sheet words as "Google sign-in needs a client id" and points at the guide. A client whose row
//! says `testing = true` is signed out by Google every seven days; a refresh Google refuses is
//! then `NeedsReauth` with [`porter_core::ReauthReason::TestingAppExpired`].

mod probe;
mod reauth;
mod scopes;

use crate::extend::{answered_as, asked_claims};
use crate::oauth::{BrowserIssuer, Cx, Grant, OAuthSession, provider_fault, userinfo_address};
use porter_capability::{Ask, Capability, CapabilityKind, Claim, Offer, StorageScope};
use porter_core::{
    AccountLabel, Audience, Credential, EndpointUrl, Family, GrantedScopes, Limit, LimitReason,
    LoginName, ReauthReason, Restriction, SecretPurpose, ServiceEndpoint, TenantConsent, Tls,
    TokenKind, TokenLifetime, Verification,
};
use porter_http::Http;
use porter_ids::{Count, UnixSeconds, UrlScheme};
use porter_oauth::TokenResponse;
use porter_provider::{Issuer, ProviderError, ProviderSpec, Signed};
use porter_sheet::SignInFault;
use probe::probe;
use reauth::reason_of_refusal;
use scopes::Granted;

/// Google's cap on the users of an app in testing.
const TESTING_USER_CAP: u32 = 100;

/// Google's rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct Google;

/// What a Google session keeps beside its tokens.
#[derive(Debug)]
pub struct GoogleState {
    /// The scopes the last token answer said were granted.
    granted: Granted,
    /// When the person signed in, if the host knows.
    signed_in: Option<UnixSeconds>,
    /// Why the last refresh was refused.
    reauth: Option<ReauthReason>,
}

impl<H: Http> OAuthSession<Google, H> {
    /// The same session knowing the person signed in at `at`, so a refusal can be told from
    /// Google's seven days (`reason_of_refusal`). accountd does not keep that date yet
    /// (FINDINGS), so `open` leaves it unknown.
    pub fn signed_in_at(self, at: UnixSeconds) -> Self {
        self.state().extra.signed_in = Some(at);
        self
    }

    /// Why the last refresh was refused, in words for the sheet, once one has been: the account
    /// is then `NeedsReauth` and this is what to tell the person.
    pub fn reauth_reason(&self) -> Option<ReauthReason> {
        self.state().extra.reauth
    }
}

/// The APIs' bases: the fixed endpoints of the file's rows, Google's own where it names none.
#[derive(Debug, Clone)]
pub(crate) struct Apis(Vec<(Family, EndpointUrl)>);

impl Apis {
    /// The base of `family`'s API.
    pub(crate) fn base(&self, family: Family) -> EndpointUrl {
        self.0
            .iter()
            .find(|(f, _)| *f == family)
            .map(|(_, url)| url.clone())
            .or_else(|| Issuer::Google.data().api(family))
            .unwrap_or_else(|| unreachable!("{family:?} is a Google API family"))
    }
}

/// The endpoints the file's rows name.
pub(crate) fn apis_of(spec: &ProviderSpec) -> Apis {
    Apis(
        spec.capabilities
            .iter()
            .filter_map(|row| {
                let url = EndpointUrl::parse(&row.endpoint.as_ref()?.0).ok()?;
                Some((row.family, url))
            })
            .collect(),
    )
}

impl BrowserIssuer for Google {
    type State = GoogleState;

    fn issuer(&self) -> Issuer {
        Issuer::Google
    }

    fn open_state(&self, credential: &Credential) -> GoogleState {
        let granted = match credential {
            Credential::OAuth { granted, .. } => Granted::recorded(granted),
            _ => Granted::of(Vec::new()),
        };
        GoogleState {
            granted,
            signed_in: None,
            reauth: None,
        }
    }

    /// A person who said no cancelled; a Workspace that wants an administrator's approval
    /// (`admin_policy_enforced`) or a client that is not allowed this person (`access_denied`
    /// with a test-user list) forbids; the rest were refused.
    fn refusal(&self, error: &str) -> SignInFault {
        match error {
            "access_denied" => SignInFault::Cancelled,
            e if e.contains("admin") || e.contains("policy") => SignInFault::Forbidden,
            _ => SignInFault::Refused,
        }
    }

    async fn signed<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: Grant,
        tokens: TokenResponse,
    ) -> Result<Signed, SignInFault> {
        let now = (cx.env.clock)();
        // `access_type=offline` with `prompt=consent` (a client's first sign-in for this
        // identity) makes Google send one; without it the account would stop working in an hour.
        // After a first sign-in the host keeps the refresh token it has (`SignedInBefore`), but
        // Google may still send none, which is read as unreadable here: the host then signs in
        // again as a first one.
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let granted = Granted::of(tokens.granted_scopes());
        let token = tokens.access_token.expose().to_owned();
        let userinfo = cx
            .env
            .userinfo_of(Issuer::Google)
            .ok_or(SignInFault::Unreadable)?;
        let address = userinfo_address(&*cx.env.http, &userinfo, &token)
            .await
            .map_err(provider_fault)?;
        let claims = probe(
            &*cx.env.http,
            cx.spec,
            &address,
            &token,
            (&granted, grant.traits.byo),
        )
        .await
        .map_err(provider_fault)?;
        let apis = apis_of(cx.spec);
        let endpoints = endpoints_for(cx.spec, &apis, (&claims, &granted), &address);
        let restriction = restriction_for(grant.traits.testing, &claims, now);
        Ok(Signed::new(
            AccountLabel(address),
            vec![(
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    expires_at: tokens.expires_at(now),
                    access: tokens.access_token,
                    refresh,
                    granted: granted.record(),
                },
            )],
            claims,
            endpoints,
            restriction,
        ))
    }

    /// Refused if the person unticked the access; else who answered. Google's answer lists
    /// everything the refresh token holds, what was granted before included.
    async fn extended<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: &Grant,
        tokens: TokenResponse,
        stored: &[ServiceEndpoint],
        kinds: &[CapabilityKind],
    ) -> Result<Signed, SignInFault> {
        let now = (cx.env.clock)();
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let granted = Granted::of(tokens.granted_scopes());
        if !granted.holds_all(&Issuer::Google.data().on_use_scopes(kinds)) {
            return Err(SignInFault::Refused);
        }
        let userinfo = cx
            .env
            .userinfo_of(Issuer::Google)
            .ok_or(SignInFault::Unreadable)?;
        let address = userinfo_address(&*cx.env.http, &userinfo, tokens.access_token.expose())
            .await
            .map_err(provider_fault)?;
        Ok(Signed::new(
            AccountLabel(address.clone()),
            vec![(
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    expires_at: tokens.expires_at(now),
                    access: tokens.access_token,
                    refresh,
                    granted: granted.record(),
                },
            )],
            asked_claims(cx.spec, kinds),
            answered_as(stored, &address),
            restriction_for(grant.traits.testing, &[], now),
        ))
    }

    async fn discover<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> Result<Vec<Claim>, ProviderError> {
        let token = session.mint(None).await?;
        let address = session.address().await?;
        let granted = session.state().extra.granted.clone();
        probe(
            &*session.env().http,
            session.spec(),
            &address,
            token.value.expose(),
            (&granted, session.byo()),
        )
        .await
    }

    async fn identify<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> Result<String, ProviderError> {
        let token = session.mint(None).await?;
        let userinfo = session
            .env()
            .userinfo_of(Issuer::Google)
            .ok_or(ProviderError::Unreadable)?;
        userinfo_address(&*session.env().http, &userinfo, token.value.expose()).await
    }

    /// The form of token `audience` takes (whole grant), or `Forbidden` for one that is not the
    /// account's.
    fn porter_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
    ) -> Result<(Option<String>, TokenKind), ProviderError> {
        let wanted = audience.0.trim_end_matches('/');
        if wanted == "imap" || wanted == "smtp" {
            return match session.byo() {
                true => Ok((None, TokenKind::Xoauth2)),
                false => Err(ProviderError::Forbidden),
            };
        }
        match names_a_row(session.spec(), wanted) {
            true => Ok((None, TokenKind::Bearer)),
            false => Err(ProviderError::Forbidden),
        }
    }

    /// A token refreshed with the scopes of `kind` alone. A scope the last token answer said was
    /// not granted is not asked: the refresh may only narrow the grant. The row is the one of the
    /// grant's `reach`: a sign-in row serves it (a Storage grant of the app folder,
    /// `drive.appdata`), and one the person unticked at sign-in is `Forbidden`; an on-use row (a
    /// Storage grant of the whole store, `drive`) is `NeedsConsent` until the account was given
    /// that access.
    fn grant_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> Result<(Option<String>, TokenKind), ProviderError> {
        let (family, form) = grant_target(session, audience, kind)?;
        let ask = session.spec().ask_for(kind, reach).unwrap_or(Ask::SignIn);
        let scopes = Issuer::Google.data().grant_scopes(kind, family, ask);
        if scopes.is_empty() {
            return Err(ProviderError::Forbidden);
        }
        if !session.state().extra.granted.admits(&scopes, ask) {
            return Err(match ask {
                Ask::OnUse => ProviderError::NeedsConsent,
                Ask::SignIn => ProviderError::Forbidden,
            });
        }
        Ok((Some(scopes.join(" ")), form))
    }

    /// Only the whole grant's answer says what was granted; a narrowed one names its own. A
    /// refresh that went through clears the reason an earlier one was refused.
    fn answered(
        &self,
        state: &mut GoogleState,
        _current: &GrantedScopes,
        scope: Option<&str>,
        tokens: &TokenResponse,
    ) -> GrantedScopes {
        state.reauth = None;
        if scope.is_none() && !tokens.granted_scopes().is_empty() {
            state.granted = Granted::of(tokens.granted_scopes());
        }
        state.granted.record()
    }

    fn refused(&self, state: &mut GoogleState, testing: bool, now: UnixSeconds) {
        state.reauth = reason_of_refusal(testing, state.signed_in, now);
    }
}

/// Whether a row of the file names `wanted`: its family's word, or its endpoint.
fn names_a_row(spec: &ProviderSpec, wanted: &str) -> bool {
    spec.capabilities.iter().any(|row| {
        porter_ids::word::word(&row.family) == wanted
            || row
                .endpoint
                .as_ref()
                .is_some_and(|e| e.0.trim_end_matches('/') == wanted)
    })
}

/// The family a grant for `kind` reaches at `audience` and the form of its token, or
/// `Forbidden` when no row of that kind names the audience (mail's IMAP and SMTP only for a
/// person's own client).
fn grant_target<H>(
    session: &OAuthSession<Google, H>,
    audience: &Audience,
    kind: CapabilityKind,
) -> Result<(Family, TokenKind), ProviderError> {
    let wanted = audience.0.trim_end_matches('/');
    let mail = |family| match (kind, session.byo()) {
        (CapabilityKind::Mail, true) => Ok((family, TokenKind::Xoauth2)),
        _ => Err(ProviderError::Forbidden),
    };
    match wanted {
        "imap" => return mail(Family::Imap),
        "smtp" => return mail(Family::Smtp),
        _ => {}
    }
    session
        .spec()
        .capabilities
        .iter()
        .filter(|row| row.capability.kind() == kind)
        .find(|row| {
            porter_ids::word::word(&row.family) == wanted
                || row
                    .endpoint
                    .as_ref()
                    .is_some_and(|e| e.0.trim_end_matches('/') == wanted)
        })
        .map(|row| (row.family, TokenKind::Bearer))
        .ok_or(ProviderError::Forbidden)
}

/// What limits the account: a client in testing is unverified (100 users, and a sign-in that
/// lasts seven days), and the Drive and Photos scopes are narrow by design (R3, R4).
fn restriction_for(testing: bool, claims: &[Claim], now: UnixSeconds) -> Restriction {
    let present = |kind| {
        claims
            .iter()
            .any(|c| matches!(&c.offer, Offer::Present(cap) if cap.kind() == kind))
    };
    let mut limits = Vec::new();
    if present(CapabilityKind::Storage) {
        limits.push(Limit::new(
            CapabilityKind::Storage,
            LimitReason::AppFolderOnly,
        ));
    }
    if present(CapabilityKind::Photos) {
        limits.push(Limit::new(CapabilityKind::Photos, LimitReason::PickerOnly));
    }
    let (verification, token_lifetime) = match testing {
        true => (
            Verification::Unverified {
                user_cap: Count(TESTING_USER_CAP),
            },
            TokenLifetime::SevenDays,
        ),
        false => (Verification::Verified, TokenLifetime::Standard),
    };
    let restriction = Restriction::none()
        .with_verification(verification)
        .with_token_lifetime(token_lifetime)
        .with_consent(TenantConsent::User)
        .with_limits(limits);
    // Only a sign-in that expires needs its date.
    match token_lifetime == TokenLifetime::SevenDays {
        true => restriction.with_signed_in(now),
        false => restriction,
    }
}

/// The servers of the account: Gmail's IMAP and SMTP when mail is there, and the API of every
/// other service that is.
fn endpoints_for(
    spec: &ProviderSpec,
    apis: &Apis,
    (claims, granted): (&[Claim], &Granted),
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
        if let (true, Some(smtp)) = (mail.send, Issuer::Google.data().api(Family::Smtp)) {
            out.push(endpoint(Family::Smtp, smtp));
        }
    }
    for row in spec.sign_in_rows() {
        let api = !matches!(row.family, Family::Imap | Family::Smtp);
        if api
            && present(row.capability.kind()).is_some()
            && row.endpoint.is_some()
            && granted.allows(row.family)
        {
            out.push(endpoint(row.family, apis.base(row.family)));
        }
    }
    out
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
            assert_eq!(Google.refusal(error), *want, "{error}");
        }
    }

    #[test]
    fn a_client_in_testing_is_unverified_for_a_hundred_users_and_seven_days() {
        let testing = restriction_for(true, &[], UnixSeconds(1_000));
        assert_eq!(
            testing.verification,
            Verification::Unverified {
                user_cap: Count(100)
            }
        );
        assert_eq!(testing.token_lifetime, TokenLifetime::SevenDays);
        assert_eq!(testing.signed_in, Some(UnixSeconds(1_000)));
        let verified = restriction_for(false, &[], UnixSeconds(1_000));
        assert_eq!(verified.verification, Verification::Verified);
        assert_eq!(verified.token_lifetime, TokenLifetime::Standard);
        assert_eq!(verified.signed_in, None);
    }
}
