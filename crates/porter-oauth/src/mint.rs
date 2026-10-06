//! OpenRouter's key mint (PLAN §2.6 `OAuthMintsKey`): the PKCE code the page sent back is posted
//! to `/api/v1/auth/keys` and answered with an API key. The code lives ten minutes. The spend cap
//! is porter's own (R10), not something the issuer enforces.

use crate::exchange::ExchangeFault;
use crate::loopback::AuthCode;
use crate::pkce::Pkce;
use porter_core::{Credential, SecretText};
use porter_http::{Http, HttpRequest, Method};
use porter_provider::IssuerEndpoints;
use serde::Deserialize;

#[derive(Deserialize)]
struct Minted {
    key: SecretText,
}

/// Exchanges `code` (with the sign-in's verifier) for an API key. `endpoints.token` is the mint
/// URL.
pub async fn mint_key<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    pkce: &Pkce,
    code: &AuthCode,
) -> Result<Credential, ExchangeFault> {
    let body = serde_json::json!({
        "code": code.0.expose(),
        "code_verifier": pkce.verifier.expose(),
        "code_challenge_method": "S256",
    });
    let request = HttpRequest::to(Method::Post, &endpoints.token)
        .map_err(|_| ExchangeFault::Unreachable)?
        .with_header("Content-Type", "application/json")
        .with_header("Accept", "application/json")
        .with_body(body.to_string());
    let response = http
        .send(request)
        .await
        .map_err(|_| ExchangeFault::Unreachable)?;
    match response.status.0 {
        200..=299 => serde_json::from_slice::<Minted>(&response.body)
            .map(|m| Credential::ApiKey(m.key))
            .map_err(|_| ExchangeFault::Unreadable),
        // A wrong, spent or expired code.
        400 | 401 | 403 => Err(ExchangeFault::Refused),
        429 | 500..=599 => Err(ExchangeFault::Unreachable),
        _ => Err(ExchangeFault::Unreadable),
    }
}
