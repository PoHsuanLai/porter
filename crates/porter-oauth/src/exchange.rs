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
    /// Seconds the access token lasts. RFC 6749 section 5.1 makes `expires_in` RECOMMENDED, not
    /// required, so an answer without it reads as [`DEFAULT_EXPIRES_IN_SECONDS`] (one hour, what
    /// mailo assumed).
    #[serde(default = "default_expires_in")]
    pub expires_in: u32,
}

/// What `expires_in` is taken to be when the issuer's answer leaves it out: one hour.
pub const DEFAULT_EXPIRES_IN_SECONDS: u32 = 3600;

fn default_expires_in() -> u32 {
    DEFAULT_EXPIRES_IN_SECONDS
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

/// What an issuer wrote in its error body: the `error` code and the human `error_description`
/// (RFC 6749 section 5.2), for a host to show. Both are untrusted text from the network, so each
/// has its control characters stripped and is cut to [`SAYS_MAX_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuerSays {
    /// The `error` code, e.g. `invalid_grant`.
    pub error: Option<String>,
    /// The `error_description`.
    pub description: Option<String>,
}

/// The most characters kept of each of an issuer's `error` and `error_description`.
pub const SAYS_MAX_CHARS: usize = 200;

/// An [`ExchangeFault`] with what the issuer said about it, from the `*_detailed` calls. The
/// plain calls return the fault alone, unchanged; a host that wants to show the issuer's reason
/// (mailo does) calls the detailed one and reads `says`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{fault}")]
pub struct ExchangeFailure {
    /// What it amounts to; the same value the plain call returns.
    pub fault: ExchangeFault,
    /// The issuer's own words, when its answer had an OAuth error body.
    pub says: Option<IssuerSays>,
}

impl From<ExchangeFault> for ExchangeFailure {
    fn from(fault: ExchangeFault) -> Self {
        Self { fault, says: None }
    }
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
    exchange_code_scoped_detailed(http, endpoints, client, pkce, code, redirect, scope)
        .await
        .map_err(|failure| failure.fault)
}

/// [`exchange_code_scoped`] with the issuer's `error` and `error_description` on a failure.
pub async fn exchange_code_scoped_detailed<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    pkce: &Pkce,
    code: &AuthCode,
    redirect: &str,
    scope: Option<&str>,
) -> Result<TokenResponse, ExchangeFailure> {
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code.0.expose()),
        ("redirect_uri", redirect),
        ("code_verifier", pkce.verifier.expose()),
    ];
    pairs.extend(scope.map(|s| ("scope", s)));
    token_call_detailed(http, endpoints, client, pairs).await
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
    refresh_scoped_detailed(http, endpoints, client, refresh_token, scope)
        .await
        .map_err(|failure| failure.fault)
}

/// [`refresh_scoped`] with the issuer's `error` and `error_description` on a failure.
pub async fn refresh_scoped_detailed<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    refresh_token: &SecretText,
    scope: Option<&str>,
) -> Result<TokenResponse, ExchangeFailure> {
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.expose()),
    ];
    pairs.extend(scope.map(|s| ("scope", s)));
    token_call_detailed(http, endpoints, client, pairs).await
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
    let response = form::send_form(http, url, &[("token", token.expose())]).await?;
    match response.status.0 {
        200..=299 => Ok(()),
        401 => Err(ExchangeFault::Refused),
        429 | 500..=599 => Err(ExchangeFault::Unreachable),
        _ => Err(ExchangeFault::Unreadable),
    }
}

pub(crate) async fn token_call_detailed<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    mut pairs: Vec<(&str, &str)>,
) -> Result<TokenResponse, ExchangeFailure> {
    pairs.push(("client_id", client.client_id.0.as_str()));
    pairs.extend(
        client
            .client_secret
            .as_ref()
            .map(|s| ("client_secret", s.expose())),
    );
    let response = form::send_form(http, &endpoints.token, &pairs).await?;
    read_tokens_detailed(&response)
}

/// A token endpoint's answer as tokens, or the fault it amounts to.
pub(crate) fn read_tokens(response: &HttpResponse) -> Result<TokenResponse, ExchangeFault> {
    read_tokens_detailed(response).map_err(|failure| failure.fault)
}

/// [`read_tokens`] keeping what the issuer said.
pub(crate) fn read_tokens_detailed(
    response: &HttpResponse,
) -> Result<TokenResponse, ExchangeFailure> {
    let fault = match response.status.0 {
        200..=299 => {
            return serde_json::from_slice(&response.body)
                .map_err(|_| ExchangeFault::Unreadable.into());
        }
        401 => ExchangeFault::Refused,
        400 if form::oauth_error(response).as_deref() == Some("invalid_grant") => {
            ExchangeFault::Refused
        }
        429 | 500..=599 => ExchangeFault::Unreachable,
        _ => ExchangeFault::Unreadable,
    };
    Err(ExchangeFailure {
        fault,
        says: form::issuer_says(response),
    })
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
        assert_eq!(
            seen[0].url.as_str(),
            Issuer::Google.endpoints().token.as_str()
        );
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

    #[tokio::test]
    async fn an_answer_without_expires_in_lasts_an_hour() {
        let now = UnixSeconds(1_000);
        let cases = [
            ("absent", r#"{"access_token":"a"}"#, 3600),
            (
                "absent, rotated",
                r#"{"access_token":"a","refresh_token":"r2"}"#,
                3600,
            ),
            ("given", r#"{"access_token":"a","expires_in":60}"#, 60),
            (
                "zero is the issuer's word",
                r#"{"access_token":"a","expires_in":0}"#,
                0,
            ),
        ];
        for (name, body, seconds) in cases {
            let tokens = refresh_with(vec![answer(200, body)]).await.expect(name);
            assert_eq!(tokens.expires_in, seconds, "{name}");
            assert_eq!(
                tokens.expires_at(now),
                UnixSeconds(1_000 + i64::from(seconds)),
                "{name}"
            );
        }
        assert_eq!(DEFAULT_EXPIRES_IN_SECONDS, 3600);
        // Still not an answer: no access token.
        let bare = refresh_with(vec![answer(200, r#"{"expires_in":60}"#)]).await;
        assert_eq!(bare, Err(ExchangeFault::Unreadable));
    }

    #[tokio::test]
    async fn a_failure_carries_what_the_issuer_said_tidied_and_capped() {
        let long = "x".repeat(500);
        let body = format!(
            r#"{{"error":"invalid_grant","error_description":"Token\n\u001b[31mexpired {long}"}}"#
        );
        let http = Scripted::new(vec![answer(400, &body)]);
        let failure = refresh_scoped_detailed(
            &http,
            &Issuer::Microsoft.endpoints(),
            &client(None),
            &SecretText::new("r"),
            None,
        )
        .await
        .expect_err("refused");
        assert_eq!(failure.fault, ExchangeFault::Refused);
        let says = failure.says.expect("says");
        assert_eq!(says.error.as_deref(), Some("invalid_grant"));
        let text = says.description.expect("description");
        assert!(text.starts_with("Token[31mexpired x"), "{text}");
        assert!(!text.chars().any(char::is_control));
        assert_eq!(text.chars().count(), SAYS_MAX_CHARS);
    }

    #[tokio::test]
    async fn the_issuers_words_come_only_from_the_detailed_calls_and_only_when_there_are_some() {
        let cases: Vec<(&str, Result<HttpResponse, HttpError>, ExchangeFault, bool)> = vec![
            (
                "401 with a body",
                answer(
                    401,
                    r#"{"error":"invalid_token","error_description":"gone"}"#,
                ),
                ExchangeFault::Refused,
                true,
            ),
            (
                "401 without",
                answer(401, ""),
                ExchangeFault::Refused,
                false,
            ),
            (
                "html",
                answer(400, "<html>"),
                ExchangeFault::Unreadable,
                false,
            ),
            (
                "blank description",
                answer(400, r#"{"error_description":" \n "}"#),
                ExchangeFault::Unreadable,
                false,
            ),
            (
                "no route",
                Err(HttpError::Unreachable),
                ExchangeFault::Unreachable,
                false,
            ),
        ];
        for (name, scripted, fault, has_says) in cases {
            let http = Scripted::new(vec![scripted.clone()]);
            let failure = refresh_scoped_detailed(
                &http,
                &Issuer::Microsoft.endpoints(),
                &client(None),
                &SecretText::new("r"),
                None,
            )
            .await
            .expect_err(name);
            assert_eq!(
                (failure.fault, failure.says.is_some()),
                (fault, has_says),
                "{name}"
            );
            // The plain call is unchanged: the fault alone.
            assert_eq!(refresh_with(vec![scripted]).await, Err(fault), "{name}");
        }
    }

    #[tokio::test]
    async fn the_detailed_code_exchange_reads_the_same_way() {
        use crate::loopback::AuthCode;
        let http = Scripted::new(vec![answer(
            400,
            r#"{"error":"invalid_grant","error_description":"code spent"}"#,
        )]);
        let pkce = Pkce::from_random([1; 32], [2; 16]);
        let failure = exchange_code_scoped_detailed(
            &http,
            &Issuer::Microsoft.endpoints(),
            &client(None),
            &pkce,
            &AuthCode(SecretText::new("c")),
            "http://127.0.0.1:1/",
            None,
        )
        .await
        .expect_err("refused");
        assert_eq!(failure.fault, ExchangeFault::Refused);
        assert_eq!(
            failure.says.and_then(|s| s.description).as_deref(),
            Some("code spent")
        );
    }
}
