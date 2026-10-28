//! Credentials and where they are filed (design/31 §4.6). These values live in accountd and
//! the secret store only: no wire type carries a [`Credential`].

use crate::AccountId;
use porter_capability::CapabilityKind;
use porter_ids::UnixSeconds;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::fmt;
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Secret text, held in a [`secrecy::SecretString`]: its `Debug` never shows it, so it cannot
/// leak into a log, a panic or an error; its bytes are overwritten when it is dropped, so a freed
/// buffer does not keep it; two of them compare in constant time (`subtle`). It has no
/// `Serialize`: a value that must be written goes through [`write_secret`] or
/// [`write_secret_option`], named at the field that writes it. (A copy made by growing a `String`
/// before it was wrapped, or by a caller of [`SecretText::expose`], is that code's to wipe.)
#[derive(Clone)]
pub struct SecretText(SecretString);

impl SecretText {
    /// Wraps a secret.
    pub fn new(text: impl Into<String>) -> Self {
        Self(SecretString::from(text.into()))
    }

    /// The secret itself, for the one call that must present it.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl PartialEq for SecretText {
    fn eq(&self, other: &Self) -> bool {
        self.expose()
            .as_bytes()
            .ct_eq(other.expose().as_bytes())
            .into()
    }
}

impl Eq for SecretText {}

impl Zeroize for SecretText {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl ZeroizeOnDrop for SecretText {}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        SecretString::deserialize(deserializer).map(Self)
    }
}

/// Writes a secret as its text. The one way a [`SecretText`] is serialised, named by
/// `#[serde(serialize_with = "...")]` at each field that must carry it (the stored credential,
/// an issued token, a typed sheet answer, `clients.toml`).
pub fn write_secret<S: serde::Serializer>(
    secret: &SecretText,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.expose())
}

/// [`write_secret`] for an optional secret.
pub fn write_secret_option<S: serde::Serializer>(
    secret: &Option<SecretText>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match secret {
        Some(secret) => serializer.serialize_some(secret.expose()),
        None => serializer.serialize_none(),
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
    Password(#[serde(serialize_with = "write_secret")] SecretText),
    /// OAuth tokens.
    #[serde(rename = "oauth")]
    OAuth {
        /// The current access token.
        #[serde(serialize_with = "write_secret")]
        access: SecretText,
        /// The refresh token; it never leaves accountd.
        #[serde(serialize_with = "write_secret")]
        refresh: SecretText,
        /// When `access` expires.
        expires_at: UnixSeconds,
        /// The scopes the refresh token was granted, as the issuer last said. A credential
        /// stored before porter kept them has none (`GrantedScopes::unknown`).
        #[serde(default, skip_serializing_if = "GrantedScopes::is_unknown")]
        granted: GrantedScopes,
    },
    /// An API key.
    ApiKey(#[serde(serialize_with = "write_secret")] SecretText),
    /// A bearer token the person pasted (a JMAP API token): presented as a bearer, with no
    /// refresh. It is filed where a password would be, so an account has one such secret.
    Bearer(#[serde(serialize_with = "write_secret")] SecretText),
    /// An access key pair.
    KeyPair {
        /// The public half (an access key id).
        access_key: String,
        /// The secret half.
        #[serde(serialize_with = "write_secret")]
        secret: SecretText,
    },
}

/// The scopes an OAuth refresh token was granted, as the issuer's token answers said (their
/// `scope`), kept with the token so they are replaced with it: a sign-in again, or one that asks
/// for more, files both at once. A record from before porter kept them is unknown, which a family
/// reads as "what sign-in asks was granted, and nothing asked later".
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantedScopes(Option<Vec<String>>);

impl GrantedScopes {
    /// Not known: a credential stored before porter kept its scopes, or an issuer that does not
    /// say.
    pub fn unknown() -> Self {
        Self(None)
    }

    /// These scopes, once each, in the order given.
    pub fn of<I, S>(scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut held = Vec::new();
        for scope in scopes.into_iter().map(Into::into) {
            if !held.contains(&scope) {
                held.push(scope);
            }
        }
        Self(Some(held))
    }

    /// Whether nothing is known.
    pub fn is_unknown(&self) -> bool {
        self.0.is_none()
    }

    /// The scopes, when known.
    pub fn scopes(&self) -> Option<&[String]> {
        self.0.as_deref()
    }

    /// Whether `scope` was granted, or `None` when that is not known.
    pub fn holds(&self, scope: &str) -> Option<bool> {
        self.0.as_ref().map(|held| held.iter().any(|s| s == scope))
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::Password(_) => f.write_str("Credential::Password(<redacted>)"),
            Credential::ApiKey(_) => f.write_str("Credential::ApiKey(<redacted>)"),
            Credential::Bearer(_) => f.write_str("Credential::Bearer(<redacted>)"),
            Credential::OAuth {
                expires_at,
                granted,
                ..
            } => f
                .debug_struct("Credential::OAuth")
                .field("access", &"<redacted>")
                .field("refresh", &"<redacted>")
                .field("expires_at", expires_at)
                .field("granted", granted)
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
            granted: GrantedScopes::unknown(),
        };
        let shown = format!("{credential:?} {:?}", SecretText::new("pw-xyz"));
        assert!(!shown.contains("xyz"), "{shown}");
    }

    #[test]
    fn a_credential_stored_before_scopes_were_kept_reads_as_unknown_and_writes_back_the_same() {
        let old = r#"{"kind":"oauth","v":{"access":"a","refresh":"r","expires_at":10}}"#;
        let read: Credential = serde_json::from_str(old).expect("an old credential");
        let Credential::OAuth { granted, .. } = &read else {
            panic!("{read:?}");
        };
        assert!(granted.is_unknown());
        assert_eq!(granted.holds("files"), None);
        assert_eq!(serde_json::to_string(&read).expect("json"), old);
        let new = Credential::OAuth {
            access: SecretText::new("a"),
            refresh: SecretText::new("r"),
            expires_at: UnixSeconds(10),
            granted: GrantedScopes::of(["files", "mail", "files"]),
        };
        let text = serde_json::to_string(&new).expect("json");
        assert!(text.contains(r#""granted":["files","mail"]"#), "{text}");
        let back: Credential = serde_json::from_str(&text).expect("read back");
        assert_eq!(back, new);
        let Credential::OAuth { granted, .. } = back else {
            unreachable!()
        };
        assert_eq!(granted.holds("mail"), Some(true));
        assert_eq!(granted.holds("calendar"), Some(false));
    }

    /// `SecretText` has no `Serialize`: a writer names `write_secret`. (Method resolution picks
    /// the inherent constant only for a type that is `Serialize`.)
    #[test]
    fn a_secret_text_does_not_implement_serialize() {
        struct Probe<T>(std::marker::PhantomData<T>);
        trait Otherwise {
            const SERIALIZE: bool = false;
        }
        impl<T> Otherwise for Probe<T> {}
        impl<T: Serialize> Probe<T> {
            const SERIALIZE: bool = true;
        }
        const { assert!(!Probe::<SecretText>::SERIALIZE) };
        const { assert!(Probe::<String>::SERIALIZE) };
    }

    #[test]
    fn secrets_compare_by_their_text() {
        assert_eq!(SecretText::new("pw"), SecretText::new("pw"));
        assert_ne!(SecretText::new("pw"), SecretText::new("pw2"));
        assert_ne!(SecretText::new("pw"), SecretText::new("px"));
    }

    /// `SecretText` wipes itself when dropped: it is `ZeroizeOnDrop`, and `zeroize`, which its
    /// `Drop` calls, leaves nothing of the secret.
    #[test]
    fn a_secret_is_wiped_when_dropped() {
        fn wiped_on_drop<T: ZeroizeOnDrop>() {}
        wiped_on_drop::<SecretText>();
        let mut secret = SecretText::new("pw-xyz");
        secret.zeroize();
        assert!(
            secret.expose().bytes().all(|b| b == 0),
            "{:?}",
            secret.expose()
        );
    }
}
