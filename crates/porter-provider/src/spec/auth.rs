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

/// An OAuth issuer: its endpoints are code, its client ids are deployment configuration, one
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
    /// Where a device code is requested, if the issuer has the device flow.
    pub device: Option<EndpointUrl>,
}

impl Issuer {
    /// The endpoints the issuer publishes. Microsoft's are the `common` tenant (work, school and
    /// personal accounts); a client registration that wants `organizations` or `consumers`
    /// carries its own `endpoints`. Google's are public facts and carry no client id. Dropbox,
    /// Box and Fastmail have no RFC 7009 revoke (Dropbox's wants a bearer header, Box's the client
    /// secret), so theirs is absent; Microsoft has none at all.
    pub fn endpoints(self) -> IssuerEndpoints {
        let rows = match self {
            Issuer::Google => (
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                Some("https://oauth2.googleapis.com/revoke"),
                Some("https://oauth2.googleapis.com/device/code"),
            ),
            Issuer::Microsoft => (
                "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                None,
                Some("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode"),
            ),
            Issuer::Dropbox => (
                "https://www.dropbox.com/oauth2/authorize",
                "https://api.dropboxapi.com/oauth2/token",
                None,
                None,
            ),
            Issuer::Box => (
                "https://account.box.com/api/oauth2/authorize",
                "https://api.box.com/oauth2/token",
                None,
                None,
            ),
            Issuer::Fastmail => (
                "https://api.fastmail.com/oauth/authorize",
                "https://api.fastmail.com/oauth/refresh",
                None,
                None,
            ),
            // The key mint: the page the person approves on, and the call that turns the code
            // into a key.
            Issuer::OpenRouter => (
                "https://openrouter.ai/auth",
                "https://openrouter.ai/api/v1/auth/keys",
                None,
                None,
            ),
            Issuer::OpenAi => (
                "https://auth.openai.com/oauth/authorize",
                "https://auth.openai.com/oauth/token",
                None,
                None,
            ),
        };
        let url = |text: &str| {
            EndpointUrl::parse(text)
                .unwrap_or_else(|_| unreachable!("{text} is a literal endpoint URL"))
        };
        IssuerEndpoints {
            authorize: url(rows.0),
            token: url(rows.1),
            revoke: rows.2.map(url),
            device: rows.3.map(url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Issuer; 7] = [
        Issuer::Google,
        Issuer::Microsoft,
        Issuer::Dropbox,
        Issuer::Box,
        Issuer::Fastmail,
        Issuer::OpenRouter,
        Issuer::OpenAi,
    ];

    #[test]
    fn every_issuer_has_https_endpoints_and_only_known_ones_revoke_or_device() {
        for issuer in ALL {
            let e = issuer.endpoints();
            let all = [
                Some(&e.authorize),
                Some(&e.token),
                e.revoke.as_ref(),
                e.device.as_ref(),
            ];
            for url in all.into_iter().flatten() {
                assert!(url.as_str().starts_with("https://"), "{issuer:?} {url:?}");
            }
        }
        let has = |i: Issuer| {
            (
                i.endpoints().revoke.is_some(),
                i.endpoints().device.is_some(),
            )
        };
        assert_eq!(has(Issuer::Google), (true, true));
        assert_eq!(has(Issuer::Microsoft), (false, true));
        assert_eq!(has(Issuer::OpenRouter), (false, false));
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
