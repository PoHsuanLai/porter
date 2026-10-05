//! The calls to an issuer's token and revoke endpoints, over the `Http` seam. The fault mapping
//! follows mailo's refresh rules (`~/mailo/crates/mail-runtime/src/oauth.rs`): an issuer that
//! cannot be reached this minute has refused nothing, so only `invalid_grant` (or a 401) is
//! `Refused`.

use crate::form;
use crate::loopback::AuthCode;
use crate::pkce::Pkce;
use porter_core::{SecretText, UnixSeconds};
use porter_http::{Http, HttpResponse};
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

impl TokenResponse {
    /// When the access token expires, given the time of the request.
    pub fn expires_at(&self, now: UnixSeconds) -> UnixSeconds {
        UnixSeconds(now.0 + i64::from(self.expires_in))
    }
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
    exchange_code_scoped(http, endpoints, client, pkce, code, redirect, None).await
}

/// [`exchange_code`] naming the scopes to redeem the code for (see `redeem_scope`).
pub async fn exchange_code_scoped<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    pkce: &Pkce,
    code: &AuthCode,
    redirect: &str,
    scope: Option<&str>,
) -> Result<TokenResponse, ExchangeFault> {
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code.0.expose()),
        ("redirect_uri", redirect),
        ("code_verifier", pkce.verifier.expose()),
    ];
    pairs.extend(scope.map(|s| ("scope", s)));
    token_call(http, endpoints, client, pairs).await
}

/// Renews an access token from the refresh token; `Refused` for `invalid_grant`. The answer's
/// `refresh_token` is the rotated one when the issuer rotates, and absent when it does not.
pub async fn refresh<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    refresh_token: &SecretText,
    now: UnixSeconds,
) -> Result<TokenResponse, ExchangeFault> {
    let _ = now; // the answer carries `expires_in`; the caller dates it with `expires_at`
    refresh_scoped(http, endpoints, client, refresh_token, None).await
}

/// [`refresh`] for the access token of another resource: Microsoft issues one token per resource
/// and reaches the others by refreshing with their scopes.
pub async fn refresh_scoped<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    refresh_token: &SecretText,
    scope: Option<&str>,
) -> Result<TokenResponse, ExchangeFault> {
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ];
    pairs.extend(scope.map(|s| ("scope", s)));
    token_call(http, endpoints, client, pairs).await
}

/// Revokes a token at the issuer, best effort; an issuer without a revoke endpoint is a success
/// (there is nothing to ask).
pub async fn revoke<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    token: &SecretText,
) -> Result<(), ExchangeFault> {
    let Some(url) = &endpoints.revoke else {
        return Ok(());
    };
    let response = http
        .send(form::post(url, &[("token", token.expose())]))
        .await
        .map_err(|_| ExchangeFault::Unreachable)?;
    match response.status.0 {
        200..=299 => Ok(()),
        401 => Err(ExchangeFault::Refused),
        429 | 500..=599 => Err(ExchangeFault::Unreachable),
        _ => Err(ExchangeFault::Unreadable),
    }
}

pub(crate) async fn token_call<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    mut pairs: Vec<(&str, &str)>,
) -> Result<TokenResponse, ExchangeFault> {
    pairs.push(("client_id", client.client_id.0.as_str()));
    pairs.extend(
        client
            .client_secret
            .as_ref()
            .map(|s| ("client_secret", s.expose())),
    );
    let response = http
        .send(form::post(&endpoints.token, &pairs))
        .await
        .map_err(|_| ExchangeFault::Unreachable)?;
    read_tokens(&response)
}

/// A token endpoint's answer as tokens, or the fault it amounts to.
pub(crate) fn read_tokens(response: &HttpResponse) -> Result<TokenResponse, ExchangeFault> {
    match response.status.0 {
        200..=299 => serde_json::from_slice(&response.body).map_err(|_| ExchangeFault::Unreadable),
        401 => Err(ExchangeFault::Refused),
        400 if form::oauth_error(response).as_deref() == Some("invalid_grant") => {
            Err(ExchangeFault::Refused)
        }
        429 | 500..=599 => Err(ExchangeFault::Unreachable),
        _ => Err(ExchangeFault::Unreadable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted::{Scripted, answer};
    use porter_http::HttpError;
    use porter_provider::{ClientChannel, ClientId, Issuer};

    fn client(secret: Option<&str>) -> ClientEntry {
        ClientEntry {
            issuer: Issuer::Microsoft,
            channel: ClientChannel::Stable,
            client_id: ClientId("cid".into()),
            client_secret: secret.map(SecretText::new),
            endpoints: None,
        }
    }

    type Case = (
        &'static str,
        Result<HttpResponse, HttpError>,
        Result<(&'static str, Option<&'static str>), ExchangeFault>,
    );

    async fn refresh_with(
        answers: Vec<Result<HttpResponse, HttpError>>,
    ) -> Result<TokenResponse, ExchangeFault> {
        let http = Scripted::new(answers);
        refresh(
            &http,
            &Issuer::Microsoft.endpoints(),
            &client(None),
            &SecretText::new("r"),
            UnixSeconds(0),
        )
        .await
    }

    #[tokio::test]
    async fn a_refresh_answer_maps_to_tokens_or_the_fault_it_amounts_to() {
        const OK: &str = r#"{"access_token":"a","refresh_token":"r2","expires_in":3600}"#;
        let cases: Vec<Case> = vec![
            ("rotated", answer(200, OK), Ok(("a", Some("r2")))),
            (
                "not rotated",
                answer(200, r#"{"access_token":"a","expires_in":60}"#),
                Ok(("a", None)),
            ),
            (
                "invalid_grant",
                answer(400, r#"{"error":"invalid_grant"}"#),
                Err(ExchangeFault::Refused),
            ),
            ("401", answer(401, ""), Err(ExchangeFault::Refused)),
            (
                "invalid_client is not a re-sign-in",
                answer(400, r#"{"error":"invalid_client"}"#),
                Err(ExchangeFault::Unreadable),
            ),
            ("503", answer(503, "down"), Err(ExchangeFault::Unreachable)),
            ("429", answer(429, ""), Err(ExchangeFault::Unreachable)),
            (
                "no route",
                Err(HttpError::Unreachable),
                Err(ExchangeFault::Unreachable),
            ),
            (
                "timeout",
                Err(HttpError::TimedOut),
                Err(ExchangeFault::Unreachable),
            ),
            ("tls", Err(HttpError::Tls), Err(ExchangeFault::Unreachable)),
            (
                "html",
                answer(200, "<html>"),
                Err(ExchangeFault::Unreadable),
            ),
        ];
        for (name, scripted, want) in cases {
            let got = refresh_with(vec![scripted]).await;
            let got = got.map(|t| {
                (
                    t.access_token.expose().to_owned(),
                    t.refresh_token.map(|r| r.expose().to_owned()),
                )
            });
            let want = want.map(|(a, r)| (a.to_owned(), r.map(str::to_owned)));
            assert_eq!(got, want, "{name}");
        }
    }

    #[tokio::test]
    async fn the_request_is_a_form_with_the_client_and_its_secret_when_it_has_one() {
        let http = Scripted::new(vec![answer(200, r#"{"access_token":"a","expires_in":1}"#)]);
        refresh_scoped(
            &http,
            &Issuer::Google.endpoints(),
            &client(Some("app secret")),
            &SecretText::new("r&1"),
            Some("a b"),
        )
        .await
        .expect("tokens");
        let body = http.body_of(0);
        assert_eq!(
            body,
            "grant_type=refresh_token&refresh_token=r%261&scope=a%20b&client_id=cid&client_secret=app%20secret"
        );
        let seen = http.seen.lock().expect("lock");
        assert_eq!(seen[0].url, Issuer::Google.endpoints().token);
    }

    #[tokio::test]
    async fn revoke_maps_the_answer() {
        let ends = Issuer::Google.endpoints();
        let token = SecretText::new("t");
        for (status, want) in [
            (200, Ok(())),
            (503, Err(ExchangeFault::Unreachable)),
            (401, Err(ExchangeFault::Refused)),
            (404, Err(ExchangeFault::Unreadable)),
        ] {
            let http = Scripted::new(vec![answer(status, "")]);
            assert_eq!(revoke(&http, &ends, &token).await, want, "{status}");
        }
        let http = Scripted::new(vec![Err(HttpError::Unreachable)]);
        assert_eq!(
            revoke(&http, &ends, &token).await,
            Err(ExchangeFault::Unreachable)
        );
    }
}
