//! Credentials and where they are filed (design/31 §4.6). These values live in accountd and
//! the secret store only: no wire type carries a [`Credential`].

use crate::capability::CapabilityKind;
use crate::id::AccountId;
use crate::units::UnixSeconds;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Secret text: its `Debug` never shows it, so it cannot leak into a log, a panic or an error.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretText(String);

impl SecretText {
    /// Wraps a secret.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The secret itself, for the one call that must present it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where one secret is filed: an account and what the secret is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretKey {
    /// The account it belongs to.
    pub account: AccountId,
    /// What it is for.
    pub purpose: SecretPurpose,
}

/// What a secret is for. One account may hold several (IMAP and SMTP passwords differ on some
/// servers; a CardDAV password may differ from the mail one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum SecretPurpose {
    /// The account's one password or app password.
    Password,
    /// The incoming mail server's password, where it differs.
    IncomingPassword,
    /// The outgoing mail server's password, where it differs.
    OutgoingPassword,
    /// The password for one other service of the account (CardDAV for `Contacts`).
    ServicePassword(CapabilityKind),
    /// An OAuth refresh token (with the access token it last minted).
    #[serde(rename = "oauth_refresh")]
    OAuthRefresh,
    /// An API key.
    ApiKey,
    /// An access key pair's secret half.
    KeyPair,
    /// This device's private key for the desktop-own account's end-to-end encryption.
    DeviceKey,
}

/// A secret as stored. `Debug` is written by hand and redacts every secret field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Credential {
    /// A password or app password.
    Password(SecretText),
    /// OAuth tokens.
    #[serde(rename = "oauth")]
    OAuth {
        /// The current access token.
        access: SecretText,
        /// The refresh token; it never leaves accountd.
        refresh: SecretText,
        /// When `access` expires.
        expires_at: UnixSeconds,
    },
    /// An API key.
    ApiKey(SecretText),
    /// An access key pair.
    KeyPair {
        /// The public half (an access key id).
        access_key: String,
        /// The secret half.
        secret: SecretText,
    },
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::Password(_) => f.write_str("Credential::Password(<redacted>)"),
            Credential::ApiKey(_) => f.write_str("Credential::ApiKey(<redacted>)"),
            Credential::OAuth { expires_at, .. } => f
                .debug_struct("Credential::OAuth")
                .field("access", &"<redacted>")
                .field("refresh", &"<redacted>")
                .field("expires_at", expires_at)
                .finish(),
            Credential::KeyPair { access_key, .. } => f
                .debug_struct("Credential::KeyPair")
                .field("access_key", access_key)
                .field("secret", &"<redacted>")
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_a_secret() {
        let credential = Credential::OAuth {
            access: SecretText::new("access-xyz"),
            refresh: SecretText::new("refresh-xyz"),
            expires_at: UnixSeconds(10),
        };
        let shown = format!("{credential:?} {:?}", SecretText::new("pw-xyz"));
        assert!(!shown.contains("xyz"), "{shown}");
    }
}
