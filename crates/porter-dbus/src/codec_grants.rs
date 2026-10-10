//! The D-Bus shapes of a grant (`(sa{sv})`: its id, then the rest by name) and of an issued
//! token (`(ssx)`).

use crate::args::TokenArg;
use crate::json_value::{bad, from_vardict, to_vardict};
use porter_core::consent::Grant;
use porter_core::{CoreError, IssuedToken, SecretText, TokenKind, UnixSeconds};
use serde_json::{Map, Value as Json};

use crate::args::Details;

/// A grant as `(id, key, decision, scope, at)` with the id first and the rest by name.
pub fn grant_to_dbus(grant: &Grant) -> (String, Details) {
    let mut fields = match serde_json::to_value(grant) {
        Ok(Json::Object(fields)) => fields,
        _ => Map::new(),
    };
    fields.remove("id");
    (grant.id.as_str().to_owned(), to_vardict(&fields))
}

/// The grant a D-Bus value carries, or why it is not one.
pub fn grant_from_dbus(arg: (String, Details)) -> Result<Grant, CoreError> {
    let (id, details) = arg;
    let mut fields = from_vardict(&details)?;
    fields.insert("id".to_owned(), Json::String(id));
    serde_json::from_value(Json::Object(fields)).map_err(|e| bad(&format!("grant: {e}")))
}

/// An issued token as `(kind slug, value, expiry in Unix seconds)`.
pub fn token_to_dbus(token: &IssuedToken) -> TokenArg {
    let kind = serde_json::to_value(token.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    (kind, token.value.expose().to_owned(), token.expires.0)
}

/// The token a D-Bus value carries, or why it is not one.
pub fn token_from_dbus(arg: TokenArg) -> Result<IssuedToken, CoreError> {
    let (kind, value, expires) = arg;
    let kind: TokenKind =
        serde_json::from_value(Json::String(kind)).map_err(|e| bad(&format!("token kind: {e}")))?;
    Ok(IssuedToken::new(
        kind,
        SecretText::new(value),
        UnixSeconds(expires),
    ))
}
