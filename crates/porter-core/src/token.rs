//! What an app receives to act as an account: short-lived, never a refresh token (G4).

use crate::secret::SecretText;
use porter_ids::UnixSeconds;
use serde::{Deserialize, Serialize};

/// The two SASL initial responses porter presents or reads, unencoded: the protocol base64-encodes
/// them on the wire. One spelling of each, here, so a relay, a provider session and a fake server
/// cannot drift apart.
const FIELD_END: char = '\u{1}';

/// The SASL `PLAIN` initial response, `NUL user NUL password` (RFC 4616, with an empty authzid).
#[must_use]
pub fn plain(user: &str, password: &str) -> SecretText {
    SecretText::new(format!("\0{user}\0{password}"))
}

/// The user and password of a `PLAIN` initial response (`authzid NUL user NUL password`; the
/// authzid is ignored).
#[must_use]
pub fn parse_plain(raw: &str) -> Option<(&str, &str)> {
    let mut parts = raw.splitn(3, '\0');
    let (_authzid, user, password) = (parts.next()?, parts.next()?, parts.next()?);
    Some((user, password))
}

/// The SASL `XOAUTH2` initial response, `user=U ^A auth=Bearer T ^A ^A`.
#[must_use]
pub fn xoauth2(user: &str, token: &str) -> SecretText {
    SecretText::new(format!(
        "user={user}{FIELD_END}auth=Bearer {token}{FIELD_END}{FIELD_END}"
    ))
}

/// The bearer token inside an `XOAUTH2` initial response; `None` for none or an empty one.
#[must_use]
pub fn xoauth2_bearer(raw: &str) -> Option<&str> {
    let token = raw
        .split(FIELD_END)
        .find_map(|field| field.strip_prefix("auth=Bearer "))?;
    (!token.is_empty()).then_some(token)
}

/// The user and bearer token of an `XOAUTH2` initial response.
#[must_use]
pub fn parse_xoauth2(raw: &str) -> Option<(&str, &str)> {
    let user = raw
        .split(FIELD_END)
        .find_map(|field| field.strip_prefix("user="))?;
    Some((user, xoauth2_bearer(raw)?))
}

/// The service a token is for, as the provider names it (`https://graph.microsoft.com`,
/// `imap`). accountd picks the scopes from it; an audience the grant does not cover is refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Audience(pub String);

/// The form of an issued token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
#[non_exhaustive]
pub struct IssuedToken {
    /// Its form.
    pub kind: TokenKind,
    /// The token.
    #[serde(serialize_with = "crate::write_secret")]
    pub value: SecretText,
    /// When it stops working; ask again then, or on a 401.
    pub expires: UnixSeconds,
}

impl IssuedToken {
    /// A token of this form that stops working at `expires`.
    pub fn new(kind: TokenKind, value: SecretText, expires: UnixSeconds) -> Self {
        Self {
            kind,
            value,
            expires,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_is_nul_user_nul_password() {
        assert_eq!(plain("ada", "pw").expose(), "\0ada\0pw");
        assert_eq!(parse_plain("\0ada\0pw"), Some(("ada", "pw")));
        assert_eq!(parse_plain("z\0ada\0p\0w"), Some(("ada", "p\0w")));
        assert_eq!(parse_plain("\0ada"), None);
    }

    #[test]
    fn xoauth2_is_the_user_and_the_bearer_between_control_a() {
        let raw = xoauth2("ada@example.test", "tok-1");
        assert_eq!(
            raw.expose(),
            "user=ada@example.test\x01auth=Bearer tok-1\x01\x01"
        );
        assert_eq!(
            parse_xoauth2(raw.expose()),
            Some(("ada@example.test", "tok-1"))
        );
        assert_eq!(xoauth2_bearer(raw.expose()), Some("tok-1"));
        for bad in ["", "user=ada\x01\x01", "user=ada\x01auth=Bearer \x01\x01"] {
            assert_eq!(parse_xoauth2(bad), None, "{bad:?}");
        }
        assert_eq!(xoauth2_bearer("auth=Bearer t\x01"), Some("t"));
    }
}
