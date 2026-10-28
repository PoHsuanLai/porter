//! porter-oauth against porter-fake-servers' `FakeIssuer`, its scripted browser, and nothing
//! beyond loopback: a full PKCE sign-in, the redirect rules, rotation, `invalid_grant`, the
//! revoke.

use porter_core::{Credential, SecretText};
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Scheme, send};
use porter_fake_servers::{Consent, FakeIssuer, IssuerEvent, Running, TokenResult, follow};
use porter_http::http::{Request, Response};
use porter_http::{Http, HttpError};
use porter_ids::UnixSeconds;
use porter_oauth::{
    ExchangeFault, LoopbackFault, LoopbackServer, Pkce, RenewOutcome, authorize_url, exchange_code,
    renew, revoke,
};
use porter_provider::{ClientChannel, ClientEntry, ClientId, Issuer, IssuerEndpoints};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The `Http` seam over the fakes' minimal loopback client.
struct LoopbackHttp;

impl Http for LoopbackHttp {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>, HttpError> {
        let (address, target) =
            split_loopback(&request.uri().to_string()).map_err(|_| HttpError::Unreachable)?;
        let (head, body) = request.into_parts();
        let mut wire =
            porter_fake_servers::http::Request::new(head.method.as_str(), &target).with_body(body);
        for (name, value) in &head.headers {
            let value = value.to_str().map_err(|_| HttpError::Malformed)?;
            wire = wire.with_header(name.as_str(), value);
        }
        let response = send(&address, Scheme::Http, &wire)
            .await
            .map_err(|_| HttpError::Unreachable)?;
        let answer = response.headers.iter().fold(
            porter_http::http::Response::builder().status(response.status),
            |answer, (name, value)| answer.header(name.as_str(), value.as_str()),
        );
        Ok(answer.body(response.body)?)
    }
}

fn client() -> ClientEntry {
    ClientEntry {
        issuer: Issuer::Microsoft,
        channel: ClientChannel::Development,
        client_id: ClientId("client-1".into()),
        client_secret: None,
        endpoints: None,
    }
}

const SCOPES: &[&str] = &["Mail.Read", "offline_access"];

fn scopes() -> Vec<String> {
    SCOPES.iter().map(|s| (*s).to_owned()).collect()
}

type Issuer_ = Running<porter_fake_servers::IssuerHandle>;

async fn issuer() -> (Issuer_, IssuerEndpoints) {
    let running = FakeIssuer::start().await.expect("issuer");
    let endpoints = running.endpoints();
    (running, endpoints)
}

fn pkce() -> Pkce {
    Pkce::from_random([7; 32], [9; 16])
}

/// Opens the authorize URL in the scripted browser, concurrently with the listener's wait.
async fn browse(
    endpoints: &IssuerEndpoints,
    pkce: &Pkce,
) -> (
    String,
    Result<porter_oauth::AuthCode, LoopbackFault>,
    tokio::task::JoinHandle<std::io::Result<porter_fake_servers::Visit>>,
) {
    let server = LoopbackServer::bind().await.expect("bind");
    let redirect = server.redirect_uri();
    let url = authorize_url(endpoints, &client(), pkce, &redirect, &scopes());
    let browser = tokio::spawn(async move { follow(&url).await });
    let outcome = server.wait(&pkce.state).await;
    (redirect, outcome, browser)
}

async fn raw(port: u16, bytes: Vec<u8>) {
    if let Ok(mut socket) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        let _ = socket.write_all(&bytes).await;
        let mut sink = Vec::new();
        let _ = socket.read_to_end(&mut sink).await;
    }
}

fn tokens_issued(issuer: &Issuer_) -> usize {
    issuer
        .events()
        .iter()
        .filter(|e| {
            matches!(
                e,
                IssuerEvent::Token {
                    result: TokenResult::Issued { .. },
                    ..
                }
            )
        })
        .count()
}

#[tokio::test]
async fn a_full_pkce_sign_in_exchanges_the_code_the_browser_carried() {
    let (issuer, endpoints) = issuer().await;
    let pkce = pkce();
    let (redirect, outcome, browser) = browse(&endpoints, &pkce).await;
    let code = outcome.expect("the redirect's code");
    let visit = browser.await.expect("task").expect("browser");
    assert!(visit.landing.text().contains("Signed in"));

    let tokens = exchange_code(
        &LoopbackHttp,
        &endpoints,
        &client(),
        &pkce,
        &code,
        &redirect,
    )
    .await
    .expect("tokens");
    assert!(issuer.access_is_live(tokens.access_token.expose()));
    let refresh = tokens.refresh_token.as_ref().expect("a refresh token");
    assert!(issuer.refresh_is_live(refresh.expose()));
    assert_eq!(tokens.expires_in, 3600);

    // The authorize request the issuer saw was ours, with our state and no verifier in it.
    let seen = issuer.events();
    assert!(matches!(
        &seen[0],
        IssuerEvent::Authorize { client_id, state, consent: Consent::Grant, .. }
            if client_id == "client-1" && state.as_deref() == Some(pkce.state.0.as_str())
    ));

    // The code is single use: exchanging it again is `Refused`.
    let again = exchange_code(
        &LoopbackHttp,
        &endpoints,
        &client(),
        &pkce,
        &code,
        &redirect,
    )
    .await;
    assert_eq!(again, Err(ExchangeFault::Refused));
}

#[tokio::test]
async fn a_wrong_verifier_is_refused_by_the_issuer() {
    let (_issuer, endpoints) = issuer().await;
    let pkce = pkce();
    let (redirect, outcome, _browser) = browse(&endpoints, &pkce).await;
    let code = outcome.expect("code");
    let other = Pkce::from_random([1; 32], [9; 16]);
    let result = exchange_code(
        &LoopbackHttp,
        &endpoints,
        &client(),
        &other,
        &code,
        &redirect,
    )
    .await;
    assert_eq!(result, Err(ExchangeFault::Refused));
}

#[tokio::test]
async fn a_wrong_state_redirect_is_refused_and_nothing_is_exchanged() {
    let (issuer, _endpoints) = issuer().await;
    let server = LoopbackServer::bind().await.expect("bind");
    let port = server.port();
    let attacker = tokio::spawn(raw(
        port,
        b"GET /?code=stolen&state=not-ours HTTP/1.1\r\n\r\n".to_vec(),
    ));
    assert_eq!(
        server.wait(&pkce().state).await,
        Err(LoopbackFault::WrongState)
    );
    attacker.await.expect("task");
    assert_eq!(tokens_issued(&issuer), 0);
}

#[tokio::test]
async fn an_oversized_request_line_is_refused() {
    let server = LoopbackServer::bind().await.expect("bind");
    let port = server.port();
    let long = format!(
        "GET /?code={}&state=x HTTP/1.1\r\n\r\n",
        "a".repeat(32 * 1024)
    );
    let sender = tokio::spawn(raw(port, long.into_bytes()));
    assert_eq!(
        server.wait(&pkce().state).await,
        Err(LoopbackFault::Oversized)
    );
    sender.await.expect("task");
}

#[tokio::test]
async fn a_second_redirect_after_the_first_is_refused() {
    let (_issuer, endpoints) = issuer().await;
    let pkce = pkce();
    let server = LoopbackServer::bind().await.expect("bind");
    let port = server.port();
    let url = authorize_url(
        &endpoints,
        &client(),
        &pkce,
        &server.redirect_uri(),
        &scopes(),
    );
    let browser = tokio::spawn(async move { follow(&url).await });
    server.wait(&pkce.state).await.expect("the first redirect");
    browser.await.expect("task").expect("browser");
    // The listener took one request and is gone.
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_person_who_says_no_ends_the_sign_in_with_the_issuers_error() {
    let (issuer, endpoints) = issuer().await;
    issuer.set_consent(Consent::Deny);
    let pkce = pkce();
    let (_redirect, outcome, browser) = browse(&endpoints, &pkce).await;
    assert_eq!(outcome, Err(LoopbackFault::Refused("access_denied".into())));
    assert!(
        browser
            .await
            .expect("task")
            .expect("browser")
            .landing
            .text()
            .contains("declined")
    );
}

fn oauth(access: &str, refresh: &str, expires_at: i64) -> Credential {
    Credential::OAuth {
        access: SecretText::new(access),
        refresh: SecretText::new(refresh),
        expires_at: UnixSeconds(expires_at),
        granted: Default::default(),
    }
}

#[tokio::test]
async fn renewal_stores_the_rotated_refresh_token_and_spends_the_old_one() {
    let (issuer, endpoints) = issuer().await;
    let old = issuer.seed_refresh("client-1", "Mail.Read");
    let due = oauth("stale", &old, 1_000);
    let outcome = renew(
        &LoopbackHttp,
        &endpoints,
        &client(),
        &due,
        UnixSeconds(1_000),
    )
    .await;
    let RenewOutcome::Renewed(Credential::OAuth {
        access,
        refresh,
        expires_at,
        ..
    }) = outcome
    else {
        panic!("expected a renewal, got {outcome:?}");
    };
    assert_eq!(expires_at, UnixSeconds(4_600));
    assert_ne!(refresh.expose(), old, "the issuer rotated it");
    assert!(issuer.refresh_is_live(refresh.expose()) && !issuer.refresh_is_live(&old));
    assert!(issuer.access_is_live(access.expose()));

    // A fresh token is not renewed: the issuer hears nothing.
    let before = issuer.events().len();
    let fresh = oauth("good", refresh.expose(), 1_000 + 3_600);
    assert_eq!(
        renew(
            &LoopbackHttp,
            &endpoints,
            &client(),
            &fresh,
            UnixSeconds(1_000)
        )
        .await,
        RenewOutcome::Fresh
    );
    assert_eq!(issuer.events().len(), before);
}

#[tokio::test]
async fn invalid_grant_is_needs_reauth() {
    let (issuer, endpoints) = issuer().await;
    let token = issuer.seed_refresh("client-1", "Mail.Read");
    issuer.refuse_refreshes(1);
    let due = oauth("stale", &token, 0);
    let outcome = renew(&LoopbackHttp, &endpoints, &client(), &due, UnixSeconds(5)).await;
    assert_eq!(outcome, RenewOutcome::NeedsReauth);
    // An unknown refresh token is the same answer.
    let unknown = oauth("stale", "never-issued", 0);
    assert_eq!(
        renew(
            &LoopbackHttp,
            &endpoints,
            &client(),
            &unknown,
            UnixSeconds(5)
        )
        .await,
        RenewOutcome::NeedsReauth
    );
}

#[tokio::test]
async fn an_unreachable_issuer_is_offline_never_needs_reauth() {
    // A port that was bound and released: connection refused.
    let gone = {
        let issuer = FakeIssuer::bind().await.expect("bind");
        porter_fake_servers::IssuerHandle::endpoints(&issuer.handle())
    };
    let due = oauth("stale", "any", 0);
    let outcome = renew(&LoopbackHttp, &gone, &client(), &due, UnixSeconds(5)).await;
    assert_eq!(outcome, RenewOutcome::Offline);
}

#[tokio::test]
async fn revoke_asks_the_issuer_and_an_issuer_without_an_endpoint_is_a_success() {
    let (issuer, endpoints) = issuer().await;
    let token = issuer.seed_refresh("client-1", "Mail.Read");
    revoke(&LoopbackHttp, &endpoints, &SecretText::new(token.clone()))
        .await
        .expect("revoked");
    assert!(!issuer.refresh_is_live(&token));
    assert!(issuer.events().contains(&IssuerEvent::Revoke {
        token: token.clone(),
        known: true
    }));
    // Revoking again, or something unknown, is not an error (RFC 7009).
    revoke(&LoopbackHttp, &endpoints, &SecretText::new(token))
        .await
        .expect("idempotent");
    let none = IssuerEndpoints {
        revoke: None,
        ..endpoints
    };
    revoke(&LoopbackHttp, &none, &SecretText::new("x"))
        .await
        .expect("nothing to ask");
}
