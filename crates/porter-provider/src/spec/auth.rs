//! How a provider's accounts sign in.

use porter_core::AuthKind;
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
