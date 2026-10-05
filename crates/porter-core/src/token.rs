//! What an app receives to act as an account: short-lived, never a refresh token (G4).

use crate::secret::SecretText;
use crate::units::UnixSeconds;
use serde::{Deserialize, Serialize};

/// The service a token is for, as the provider names it (`https://graph.microsoft.com`,
/// `imap`). accountd picks the scopes from it; an audience the grant does not cover is refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Audience(pub String);

/// The form of an issued token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    /// An HTTP bearer access token.
    Bearer,
    /// A SASL XOAUTH2 string for IMAP and SMTP, unencoded (`user=..^Aauth=Bearer ..^A^A`); the
    /// protocol base64-encodes it on the wire.
    Xoauth2,
    /// A handle inferd resolves to an API key; the key itself never leaves the daemons.
    ApiKeyHandle,
}

/// A short-lived token issued to a granted app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedToken {
    /// Its form.
    pub kind: TokenKind,
    /// The token.
    pub value: SecretText,
    /// When it stops working; ask again then, or on a 401.
    pub expires: UnixSeconds,
}
