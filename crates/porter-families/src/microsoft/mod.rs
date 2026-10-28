//! Microsoft's rules (lane W5b): personal and work or school accounts through one multi-tenant
//! public client. Sign-in is PKCE through a loopback redirect; mail is IMAP and SMTP with XOAUTH2
//! (tokens for Exchange Online); calendar, contacts, tasks, OneNote and OneDrive are Graph,
//! probed at add time so a tenant that forbids one shows it as absent for consent (R13). Revoking
//! has no API: the person is sent to account.microsoft.com (`issuers.toml`).
//!
//! Microsoft's consent is all of what was asked or nothing, and an access token is for one
//! resource: a code is redeemed for Exchange when mail is asked, and Graph's token comes from a
//! refresh with the Graph scope. A token for a grant carries the one scope of its kind.

mod graph;

use crate::extend::{answered_as, asked_claims};
use crate::oauth::{BrowserIssuer, Cx, Grant, OAuthSession, exchange_fault, provider_fault};
use graph::{Found, probe, whoami};
use porter_capability::{Ask, Capability, CapabilityKind, Claim, Offer, StorageScope};
use porter_core::{
    AccountLabel, Audience, Credential, EndpointUrl, Family, GrantedScopes, LoginName, Restriction,
    SecretPurpose, ServiceEndpoint, TenantConsent, Tls, TokenKind,
};
use porter_http::Http;
use porter_ids::UrlScheme;
use porter_oauth::{TokenResponse, refresh_scoped};
use porter_provider::{Issuer, ProviderError, ProviderSpec, Signed};
use porter_sheet::SignInFault;

/// Microsoft's rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct Microsoft;

/// Graph's origin: the fixed endpoint of the file's Graph rows, else the data's.
pub(crate) fn graph_origin(spec: &ProviderSpec) -> EndpointUrl {
    fixed_endpoint(spec, Family::Graph)
}

/// Exchange Online's IMAP origin.
pub(crate) fn imap_origin(spec: &ProviderSpec) -> EndpointUrl {
    fixed_endpoint(spec, Family::Imap)
}

fn fixed_endpoint(spec: &ProviderSpec, family: Family) -> EndpointUrl {
    spec.capabilities
        .iter()
        .filter(|row| row.family == family)
        .find_map(|row| EndpointUrl::parse(&row.endpoint.as_ref()?.0).ok())
        .or_else(|| Issuer::Microsoft.data().api(family))
        .unwrap_or_else(|| unreachable!("{family:?} is a Microsoft API family"))
}

impl BrowserIssuer for Microsoft {
    type State = ();

    fn issuer(&self) -> Issuer {
        Issuer::Microsoft
    }

    fn open_state(&self, _credential: &Credential) {}

    /// Microsoft lets one sign-in consent to several resources (Exchange's IMAP and Graph's
    /// permissions) but issues each access token for one, and refuses to redeem such a code
    /// without being told which.
    fn redeem_scope(&self, scopes: &[String]) -> Option<String> {
        porter_oauth::redeem_scope(scopes)
    }

    /// A person who said no cancelled; a tenant that wants an administrator's approval forbids;
    /// the rest were refused.
    fn refusal(&self, error: &str) -> SignInFault {
        match error {
            "access_denied" => SignInFault::Cancelled,
            e if e.contains("consent") || e.contains("admin") => SignInFault::Forbidden,
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
        let refresh = tokens
            .refresh_token
            .clone()
            .ok_or(SignInFault::Unreadable)?;
        let (graph, refresh) = match self.redeem_scope(&grant.scopes) {
            None => (tokens, refresh),
            Some(_) => {
                let all = Issuer::Microsoft
                    .data()
                    .all_scope
                    .as_deref()
                    .ok_or(SignInFault::Unreadable)?;
                let graph = refresh_scoped(
                    &*cx.env.http,
                    &grant.endpoints,
                    &grant.client,
                    &refresh,
                    Some(all),
                )
                .await
                .map_err(exchange_fault)?;
                let rotated = graph.refresh_token.clone().unwrap_or(refresh);
                (graph, rotated)
            }
        };
        let base = graph_origin(cx.spec);
        let found = probe(&*cx.env.http, cx.spec, &base, graph.access_token.expose())
            .await
            .map_err(provider_fault)?;
        // Microsoft's consent is all that was asked or nothing, so what was asked is what the
        // refresh token holds.
        Ok(signed_from(
            cx.spec,
            found,
            Credential::OAuth {
                expires_at: graph.expires_at(now),
                access: graph.access_token,
                refresh,
                granted: GrantedScopes::of(grant.scopes),
            },
        ))
    }

    /// The refresh token reaches everything consented before too, so it holds what sign-in asks
    /// and what was asked now.
    async fn extended<H: Http + 'static>(
        &self,
        cx: &Cx<'_, H>,
        grant: &Grant,
        tokens: TokenResponse,
        stored: &[ServiceEndpoint],
        kinds: &[CapabilityKind],
    ) -> Result<Signed, SignInFault> {
        let now = (cx.env.clock)();
        let expires_at = tokens.expires_at(now);
        let refresh = tokens.refresh_token.ok_or(SignInFault::Unreadable)?;
        let address = whoami(
            &*cx.env.http,
            &graph_origin(cx.spec),
            tokens.access_token.expose(),
        )
        .await
        .map_err(provider_fault)?;
        let mut granted = Issuer::Microsoft
            .data()
            .sign_in_scopes(&cx.spec.sign_in_kinds(), false);
        granted.extend(grant.scopes.iter().cloned());
        Ok(Signed::new(
            AccountLabel(address.clone()),
            vec![(
                SecretPurpose::OAuthRefresh,
                Credential::OAuth {
                    expires_at,
                    access: tokens.access_token,
                    refresh,
                    granted: GrantedScopes::of(granted),
                },
            )],
            asked_claims(cx.spec, kinds),
            answered_as(stored, &address),
            Restriction::none(),
        ))
    }

    /// What the account's Graph services answer, with porter's own token for Graph.
    async fn discover<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> Result<Vec<Claim>, ProviderError> {
        use porter_provider::ProviderSession;
        let base = graph_origin(session.spec());
        let token = session
            .access_token(&Audience(base.as_str().to_owned()))
            .await?;
        let found = probe(
            &*session.env().http,
            session.spec(),
            &base,
            token.value.expose(),
        )
        .await?;
        Ok(found.claims)
    }

    /// The address from Graph's `/me`, with the profile scope alone: the address is all this
    /// token is for.
    async fn identify<H: Http + 'static>(
        &self,
        session: &OAuthSession<Self, H>,
    ) -> Result<String, ProviderError> {
        let base = graph_origin(session.spec());
        let scope = Issuer::Microsoft
            .data()
            .who_scope()
            .ok_or(ProviderError::Unreadable)?;
        let graph = session.mint(Some(scope)).await?;
        whoami(&*session.env().http, &base, graph.value.expose()).await
    }

    /// What an audience is refreshed with and the form of the token it gets, for porter's own
    /// use inside accountd (the add-time probe). No token minted this way leaves accountd: an
    /// app's or a relay's comes from `grant_token`.
    fn porter_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
    ) -> Result<(Option<String>, TokenKind), ProviderError> {
        let graph = graph_origin(session.spec());
        audience_scope(audience, graph.as_str())
            .map(|(scope, form)| (Some(scope), form))
            .ok_or(ProviderError::Forbidden)
    }

    /// The row is the one of the grant's `reach`: a Storage grant of the app folder is served by
    /// the sign-in row, one of the whole store by the on-use row, which is `NeedsConsent` until
    /// the account was given that access.
    fn grant_token<H>(
        &self,
        session: &OAuthSession<Self, H>,
        audience: &Audience,
        kind: CapabilityKind,
        reach: StorageScope,
    ) -> Result<(Option<String>, TokenKind), ProviderError> {
        let graph = graph_origin(session.spec());
        let ask = session
            .spec()
            .ask_for(kind, reach)
            .ok_or(ProviderError::Forbidden)?;
        let (scope, form) =
            grant_scope(audience, kind, ask, graph.as_str()).ok_or(ProviderError::Forbidden)?;
        if !given(session, &scope, ask) {
            return Err(ProviderError::NeedsConsent);
        }
        Ok((Some(scope), form))
    }
}

/// What an audience is refreshed with and the form of the token it gets: IMAP and SMTP take an
/// XOAUTH2 token for the Exchange resource, Graph takes a bearer for every permission consented
/// (`.default`).
fn audience_scope(audience: &Audience, graph_endpoint: &str) -> Option<(String, TokenKind)> {
    let data = Issuer::Microsoft.data();
    let exchange = |family| {
        let scopes = data.grant_scopes(CapabilityKind::Mail, family, Ask::SignIn);
        scopes
            .first()
            .map(|s| ((*s).to_owned(), TokenKind::Xoauth2))
    };
    match audience.0.as_str() {
        "imap" => exchange(Family::Imap),
        "smtp" => exchange(Family::Smtp),
        _ if is_graph(audience, graph_endpoint) => {
            data.all_scope.clone().map(|all| (all, TokenKind::Bearer))
        }
        _ => None,
    }
}

/// What a token for a grant of `kind` to `audience` is refreshed with: the kind's own scope and
/// nothing else, never `.default` (which names every Graph permission the account consented to,
/// so a calendar grant's token would reach the account's files). Microsoft's v2 token endpoint
/// takes a subset of the signed-in scopes on a refresh. Mail reaches IMAP and SMTP only; the
/// other kinds reach Graph only. `ask` is how the kind's rows are asked
/// (`ProviderSpec::ask_for`): an on-use kind's token carries its on-use permission. `None` for an
/// audience the kind does not reach.
fn grant_scope(
    audience: &Audience,
    kind: CapabilityKind,
    ask: Ask,
    graph_endpoint: &str,
) -> Option<(String, TokenKind)> {
    let (family, form) = match audience.0.as_str() {
        "imap" => (Family::Imap, TokenKind::Xoauth2),
        "smtp" => (Family::Smtp, TokenKind::Xoauth2),
        _ if is_graph(audience, graph_endpoint) => (Family::Graph, TokenKind::Bearer),
        _ => return None,
    };
    let scopes = Issuer::Microsoft.data().grant_scopes(kind, family, ask);
    scopes.first().map(|scope| ((*scope).to_owned(), form))
}

/// Whether `audience` is Graph: its slug, or the Graph endpoint the provider file names.
fn is_graph(audience: &Audience, graph_endpoint: &str) -> bool {
    audience.0 == "graph"
        || audience.0.trim_end_matches('/') == graph_endpoint.trim_end_matches('/')
}

/// Whether a grant asked as `ask` may be refreshed with `scope`. Microsoft's consent is all or
/// nothing, so what sign-in asks was given; an on-use scope only once the credential says so.
fn given<H>(session: &OAuthSession<Microsoft, H>, scope: &str, ask: Ask) -> bool {
    match (&session.state().credential, ask) {
        (_, Ask::SignIn) => true,
        (Credential::OAuth { granted, .. }, Ask::OnUse) => granted.holds(scope) == Some(true),
        _ => false,
    }
}

fn signed_from(spec: &ProviderSpec, found: Found, credential: Credential) -> Signed {
    let endpoints = endpoints_for(spec, &found);
    let restriction = Restriction::none().with_consent(match found.tenant_refused() {
        true => TenantConsent::AdminRequired,
        false => TenantConsent::User,
    });
    Signed::new(
        AccountLabel(found.address),
        vec![(SecretPurpose::OAuthRefresh, credential)],
        found.claims,
        endpoints,
        restriction,
    )
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
        out.push(endpoint(Family::Imap, imap_origin(spec)));
        if let (true, Some(smtp)) = (mail.send, Issuer::Microsoft.data().api(Family::Smtp)) {
            out.push(endpoint(Family::Smtp, smtp));
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

#[cfg(test)]
mod tests {
    use super::*;
    use CapabilityKind as K;

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
            assert_eq!(Microsoft.refusal(error), *want, "{error}");
        }
    }

    const GRAPH_DEFAULT: &str = "https://graph.microsoft.com/.default";
    const IMAP_SCOPE: &str = "https://outlook.office.com/IMAP.AccessAsUser.All";
    const SMTP_SCOPE: &str = "https://outlook.office.com/SMTP.Send";

    #[test]
    fn audiences_map_to_one_resource_and_a_token_form() {
        const G: &str = "https://graph.microsoft.com";
        const CASES: &[(&str, Option<(&str, TokenKind)>)] = &[
            ("imap", Some((IMAP_SCOPE, TokenKind::Xoauth2))),
            ("smtp", Some((SMTP_SCOPE, TokenKind::Xoauth2))),
            ("graph", Some((GRAPH_DEFAULT, TokenKind::Bearer))),
            (
                "https://graph.microsoft.com",
                Some((GRAPH_DEFAULT, TokenKind::Bearer)),
            ),
            (
                "https://graph.microsoft.com/",
                Some((GRAPH_DEFAULT, TokenKind::Bearer)),
            ),
            ("https://graph.evil.test", None),
            ("webdav", None),
        ];
        for (audience, want) in CASES {
            let got = audience_scope(&Audience((*audience).into()), G);
            assert_eq!(got, want.map(|(s, k)| (s.to_owned(), k)), "{audience}");
        }
    }

    #[test]
    fn a_grant_is_refreshed_with_its_own_kinds_scope_and_never_graphs_default() {
        const G: &str = "https://graph.microsoft.com";
        const CAL: &str = "https://graph.microsoft.com/Calendars.ReadWrite";
        const FILES: &str = "https://graph.microsoft.com/Files.ReadWrite.AppFolder";
        const WHOLE: &str = "https://graph.microsoft.com/Files.ReadWrite";
        type Want = Option<(&'static str, TokenKind)>;
        const CASES: &[(&str, K, Want)] = &[
            ("graph", K::Calendar, Some((CAL, TokenKind::Bearer))),
            (G, K::Calendar, Some((CAL, TokenKind::Bearer))),
            (
                "graph",
                K::Contacts,
                Some((
                    "https://graph.microsoft.com/Contacts.ReadWrite",
                    TokenKind::Bearer,
                )),
            ),
            (
                "graph",
                K::Tasks,
                Some((
                    "https://graph.microsoft.com/Tasks.ReadWrite",
                    TokenKind::Bearer,
                )),
            ),
            (
                "graph",
                K::Notes,
                Some((
                    "https://graph.microsoft.com/Notes.ReadWrite",
                    TokenKind::Bearer,
                )),
            ),
            ("graph", K::Storage, Some((FILES, TokenKind::Bearer))),
            ("imap", K::Mail, Some((IMAP_SCOPE, TokenKind::Xoauth2))),
            ("smtp", K::Mail, Some((SMTP_SCOPE, TokenKind::Xoauth2))),
            ("graph", K::Mail, None),
            ("imap", K::Calendar, None),
            ("smtp", K::Storage, None),
            ("graph", K::Agent, None),
            ("https://graph.evil.test", K::Calendar, None),
        ];
        for (audience, kind, want) in CASES {
            let got = grant_scope(&Audience((*audience).into()), *kind, Ask::SignIn, G);
            assert_eq!(
                got,
                want.map(|(s, k)| (s.to_owned(), k)),
                "{audience} {kind:?}"
            );
            assert!(
                got.is_none_or(|(s, _)| s != GRAPH_DEFAULT && !s.contains(' ')),
                "{audience} {kind:?}: one scope, never .default"
            );
        }
        // Every kind's grant scope is one the sign-in asked for when the kind was on.
        let data = Issuer::Microsoft.data();
        for kind in [K::Calendar, K::Contacts, K::Tasks, K::Notes, K::Storage] {
            let (scope, _) =
                grant_scope(&Audience("graph".into()), kind, Ask::SignIn, G).expect("graph kind");
            assert!(
                data.sign_in_scopes(&[kind], false).contains(&scope),
                "{kind:?}"
            );
        }
        // The whole drive is the on-use row's, and only an on-use grant carries it.
        let graph = Audience("graph".into());
        assert_eq!(
            grant_scope(&graph, K::Storage, Ask::OnUse, G),
            Some((WHOLE.to_owned(), TokenKind::Bearer))
        );
        assert_eq!(grant_scope(&graph, K::Calendar, Ask::OnUse, G), None);
        assert_eq!(
            grant_scope(&Audience("imap".into()), K::Mail, Ask::OnUse, G),
            None
        );
    }

    #[test]
    fn an_exchange_first_sign_in_redeems_for_exchange_and_a_graph_only_one_for_graph() {
        let data = Issuer::Microsoft.data();
        let mail = data.sign_in_scopes(&[K::Mail, K::Tasks], false);
        let redeemed = Microsoft.redeem_scope(&mail).expect("two resources");
        assert!(redeemed.contains("IMAP.AccessAsUser.All"));
        assert!(!redeemed.contains("graph.microsoft.com"));
        let files = data.sign_in_scopes(&[K::Storage], false);
        assert_eq!(Microsoft.redeem_scope(&files), None);
        assert_eq!(
            Microsoft.redeem_scope(&data.extend_scopes(&[K::Storage])),
            None
        );
    }
}
