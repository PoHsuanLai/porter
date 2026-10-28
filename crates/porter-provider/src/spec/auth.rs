//! How a provider's accounts sign in.

use porter_core::{AuthKind, EndpointUrl};
use serde::{Deserialize, Serialize};

/// The `[auth]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSpec {
    /// The sign-in method.
    pub kind: AuthKind,
    /// For the OAuth kinds, whose client registry and endpoints to use; absent otherwise.
    #[serde(default)]
    pub issuer: Option<Issuer>,
}

impl AuthSpec {
    /// Whether `kind` signs in through an OAuth issuer, and so needs one named.
    pub fn needs_issuer(&self) -> bool {
        matches!(
            self.kind,
            AuthKind::OAuthPkce | AuthKind::OAuthMintsKey | AuthKind::OAuthPlan
        )
    }
}

/// An OAuth issuer: its endpoints and scopes are data (`issuers.toml`), its client ids are deployment configuration, one
/// per issuer per build channel (R5), never part of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Issuer {
    /// Google.
    Google,
    /// Microsoft identity platform.
    Microsoft,
    /// Dropbox.
    Dropbox,
    /// Box.
    Box,
    /// Fastmail.
    Fastmail,
    /// OpenRouter (mints a key, R10).
    #[serde(rename = "openrouter")]
    OpenRouter,
    /// OpenAI (ChatGPT sign-in, R9).
    #[serde(rename = "openai")]
    OpenAi,
    /// Yahoo (mail over IMAP and SMTP with XOAUTH2).
    Yahoo,
}

/// Where an issuer's pages and APIs are. A client registration may point elsewhere (a sovereign
/// cloud, an inspecting proxy, a fake issuer in a test).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerEndpoints {
    /// The page the person signs in on.
    pub authorize: EndpointUrl,
    /// Where a code or a refresh token is exchanged.
    pub token: EndpointUrl,
    /// Where a grant is revoked, if the issuer has such an endpoint.
    pub revoke: Option<EndpointUrl>,
}

impl Issuer {
    /// The endpoints the issuer publishes (`issuers.toml`).
    pub fn endpoints(self) -> IssuerEndpoints {
        self.data().endpoints()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Issuer; 8] = [
        Issuer::Google,
        Issuer::Microsoft,
        Issuer::Dropbox,
        Issuer::Box,
        Issuer::Fastmail,
        Issuer::OpenRouter,
        Issuer::OpenAi,
        Issuer::Yahoo,
    ];

    #[test]
    fn every_issuer_has_https_endpoints_and_only_known_ones_revoke() {
        for issuer in ALL {
            let e = issuer.endpoints();
            let all = [Some(&e.authorize), Some(&e.token), e.revoke.as_ref()];
            for url in all.into_iter().flatten() {
                assert!(url.as_str().starts_with("https://"), "{issuer:?} {url:?}");
            }
        }
        let revokes = |i: Issuer| i.endpoints().revoke.is_some();
        assert!(revokes(Issuer::Google));
        assert!(!revokes(Issuer::Microsoft));
        assert!(!revokes(Issuer::OpenRouter));
    }

    #[test]
    fn yahoo_is_named_yahoo_in_a_file_and_signs_in_on_its_own_host() {
        let spec: AuthSpec = toml::from_str("kind = \"oauth_pkce\"\nissuer = \"yahoo\"").unwrap();
        let issuer = spec.issuer.unwrap();
        assert_eq!(issuer, Issuer::Yahoo);
        let e = issuer.endpoints();
        assert!(e.authorize.as_str().contains("login.yahoo.com"));
        assert!(e.token.as_str().contains("login.yahoo.com"));
    }

    #[test]
    fn microsoft_is_the_common_tenant() {
        let e = Issuer::Microsoft.endpoints();
        assert!(
            e.authorize
                .as_str()
                .contains("/common/oauth2/v2.0/authorize")
        );
        assert!(e.token.as_str().contains("/common/oauth2/v2.0/token"));
    }
}
