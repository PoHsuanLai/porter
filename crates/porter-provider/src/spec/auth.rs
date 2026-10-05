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
    /// The endpoints the issuer publishes.
    pub fn endpoints(self) -> IssuerEndpoints {
        let _ = self;
        todo!(
            "one row per issuer, each URL checked against the issuer's current documentation \
             (Google, Microsoft common tenant, Dropbox, Box, Fastmail, OpenRouter's key mint)"
        )
    }
}
