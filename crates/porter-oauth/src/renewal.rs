//! When an access token is renewed: a little before it expires, so an app never receives one
//! that dies in flight. The rules follow mailo's renewal (`~/mailo/crates/mail-runtime/src/renewal.rs`
//! and `oauth.rs::assess`): the refresh token is carried through when the issuer does not rotate
//! it, and only the issuer's own refusal is a reason to sign in again.

use crate::exchange::{ExchangeFault, refresh};
use porter_core::{Credential, UnixSeconds};
use porter_http::Http;
use porter_provider::{ClientEntry, IssuerEndpoints};

/// How long before expiry a token counts as due.
pub const RENEW_MARGIN_SECONDS: i64 = 60;

/// Whether a token needs renewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renewal {
    /// Good for longer than the margin.
    Fresh,
    /// Expired or about to.
    Due,
}

/// Whether a token that expires at `expires` needs renewing at `now`.
pub fn renewal(expires: UnixSeconds, now: UnixSeconds) -> Renewal {
    match expires.0 - now.0 > RENEW_MARGIN_SECONDS {
        true => Renewal::Fresh,
        false => Renewal::Due,
    }
}

/// What renewing a credential came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewOutcome {
    /// Not due; nothing was asked of the issuer.
    Fresh,
    /// The credential to hand to `ProviderSession::renewed()` and to store: a new access token,
    /// its expiry, and the refresh token (the rotated one when the issuer sent one, else the one
    /// presented, so the account is not logged out at the next expiry).
    Renewed(Credential),
    /// The issuer refused the grant (`invalid_grant`, 401): the person must sign in again.
    NeedsReauth,
    /// The issuer could not be reached or answered 5xx or nonsense: wait, never `NeedsReauth`.
    Offline,
}

/// Renews `credential` at `now` if it is due.
pub async fn renew<H: Http>(
    http: &H,
    endpoints: &IssuerEndpoints,
    client: &ClientEntry,
    credential: &Credential,
    now: UnixSeconds,
) -> RenewOutcome {
    let Credential::OAuth {
        refresh: presented,
        expires_at,
        ..
    } = credential
    else {
        return RenewOutcome::Fresh;
    };
    if renewal(*expires_at, now) == Renewal::Fresh {
        return RenewOutcome::Fresh;
    }
    match refresh(http, endpoints, client, presented, now).await {
        Ok(tokens) => RenewOutcome::Renewed(Credential::OAuth {
            expires_at: tokens.expires_at(now),
            refresh: tokens.refresh_token.unwrap_or_else(|| presented.clone()),
            access: tokens.access_token,
        }),
        Err(ExchangeFault::Refused) => RenewOutcome::NeedsReauth,
        Err(ExchangeFault::Unreachable | ExchangeFault::Unreadable) => RenewOutcome::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_due_inside_the_margin() {
        const CASES: &[(&str, i64, Renewal)] = &[
            ("an hour left", 3600, Renewal::Fresh),
            ("just outside the margin", 61, Renewal::Fresh),
            ("on the margin", 60, Renewal::Due),
            ("expiring", 1, Renewal::Due),
            ("expired", -5, Renewal::Due),
        ];
        for (name, left, want) in CASES {
            assert_eq!(
                renewal(UnixSeconds(1_000 + left), UnixSeconds(1_000)),
                *want,
                "{name}"
            );
        }
    }

    use crate::scripted::{Scripted, answer};
    use porter_core::SecretText;
    use porter_http::HttpError;
    use porter_provider::{ClientChannel, ClientId, Issuer};

    fn client() -> ClientEntry {
        ClientEntry {
            issuer: Issuer::Microsoft,
            channel: ClientChannel::Stable,
            client_id: ClientId("cid".into()),
            client_secret: None,
            endpoints: None,
        }
    }

    fn held(expires_at: i64) -> Credential {
        Credential::OAuth {
            access: SecretText::new("old-access"),
            refresh: SecretText::new("old-refresh"),
            expires_at: UnixSeconds(expires_at),
        }
    }

    async fn renewed(answer: Result<porter_http::HttpResponse, HttpError>) -> RenewOutcome {
        let http = Scripted::new(vec![answer]);
        renew(
            &http,
            &Issuer::Microsoft.endpoints(),
            &client(),
            &held(0),
            UnixSeconds(100),
        )
        .await
    }

    #[tokio::test]
    async fn the_old_refresh_token_is_kept_when_the_issuer_does_not_rotate() {
        let outcome = renewed(answer(200, r#"{"access_token":"new","expires_in":3600}"#)).await;
        assert_eq!(
            outcome,
            RenewOutcome::Renewed(Credential::OAuth {
                access: SecretText::new("new"),
                refresh: SecretText::new("old-refresh"),
                expires_at: UnixSeconds(3_700),
            })
        );
    }

    #[tokio::test]
    async fn only_the_issuers_refusal_needs_a_new_sign_in() {
        let cases = [
            (
                "invalid_grant",
                answer(400, r#"{"error":"invalid_grant"}"#),
                RenewOutcome::NeedsReauth,
            ),
            ("503", answer(503, ""), RenewOutcome::Offline),
            (
                "offline",
                Err(HttpError::Unreachable),
                RenewOutcome::Offline,
            ),
            ("gibberish", answer(200, "nope"), RenewOutcome::Offline),
        ];
        for (name, scripted, want) in cases {
            assert_eq!(renewed(scripted).await, want, "{name}");
        }
    }

    #[tokio::test]
    async fn a_credential_that_is_not_oauth_or_not_due_is_left_alone() {
        let http = Scripted::new(vec![]);
        let ends = Issuer::Microsoft.endpoints();
        let key = Credential::ApiKey(SecretText::new("k"));
        assert_eq!(
            renew(&http, &ends, &client(), &key, UnixSeconds(0)).await,
            RenewOutcome::Fresh
        );
        assert_eq!(
            renew(&http, &ends, &client(), &held(10_000), UnixSeconds(0)).await,
            RenewOutcome::Fresh
        );
        assert!(http.seen.lock().expect("lock").is_empty());
    }
}
