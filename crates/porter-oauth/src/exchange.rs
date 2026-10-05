//! The calls to an issuer's token and revoke endpoints, over the `Http` seam.

use crate::loopback::AuthCode;
use crate::pkce::Pkce;
use porter_core::{SecretText, UnixSeconds};
use porter_http::Http;
use porter_provider::{ClientEntry, IssuerEndpoints};
use serde::{Deserialize, Serialize};

/// A token endpoint's answer, as the issuer writes it. Secret fields redact in `Debug`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenResponse {
    /// The access token.
    pub access_token: SecretText,
    /// A refresh token; absent when the issuer does not rotate it.
    #[serde(default)]
    pub refresh_token: Option<SecretText>,
    /// Seconds the access token lasts.
    pub expires_in: u32,
}

/// Why an exchange failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExchangeFault {
    /// `invalid_grant`, or a 401: the credential is gone; the account needs signing in again.
    #[error("the grant was refused")]
    Refused,
    /// The issuer could not be reached, or answered 5xx.
    #[error("unreachable")]
    Unreachable,
    /// The answer was not what the issuer documents.
    #[error("unreadable answer")]
    Unreadable,
}

/// Exchanges the redirect's code for tokens, with the PKCE verifier.
pub async fn exchange_code<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    pkce: &Pkce,
    code: &AuthCode,
    redirect: &str,
) -> Result<TokenResponse, ExchangeFault> {
    let _ = (http, endpoints, client, pkce, code, redirect);
    todo!(
        "POST grant_type=authorization_code with the code, verifier and client to the token endpoint"
    )
}

/// Renews an access token from the refresh token; `Refused` for `invalid_grant`.
pub async fn refresh<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    refresh_token: &SecretText,
    now: UnixSeconds,
) -> Result<TokenResponse, ExchangeFault> {
    let _ = (http, endpoints, client, refresh_token, now);
    todo!(
        "POST grant_type=refresh_token; a 400 with invalid_grant is `Refused`, a 5xx `Unreachable`"
    )
}

/// Revokes a token at the issuer, best effort.
pub async fn revoke<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    token: &SecretText,
) -> Result<(), ExchangeFault> {
    let _ = (http, endpoints, token);
    todo!("POST the token to the revoke endpoint when the issuer has one")
}
