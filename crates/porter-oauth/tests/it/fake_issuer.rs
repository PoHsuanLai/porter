//! porter-oauth against porter-fake-servers' `FakeIssuer`, its scripted browser, and nothing
//! beyond loopback: a full PKCE sign-in, the redirect rules, rotation, `invalid_grant`, the
//! device flow, revoke and OpenRouter's key mint.

use porter_core::{Credential, EndpointUrl, SecretText, UnixSeconds};
use porter_fake::FakeAddress;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Response, Scheme, send, serve};
use porter_fake_servers::net::{Bind, Listener};
use porter_fake_servers::{Consent, FakeIssuer, IssuerEvent, Running, TokenResult, follow};
use porter_http::{Http, HttpError, HttpRequest, HttpResponse, Status};
use porter_oauth::{
    DeviceFault, ExchangeFault, LoopbackFault, LoopbackServer, Pkce, RenewOutcome, authorize_url,
    await_device, exchange_code, mint_key, openrouter_auth_url, renew, request_device_code, revoke,
};
use porter_provider::{ClientChannel, ClientEntry, ClientId, Issuer, IssuerEndpoints};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The `Http` seam over the fakes' minimal loopback client.
struct LoopbackHttp;

impl Http for LoopbackHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let (address, target) =
            split_loopback(request.url.as_str()).map_err(|_| HttpError::Unreachable)?;
        let mut wire = Request::new(request.method.token(), &target).with_body(request.body);
        for h in &request.headers {
            wire = wire.with_header(h.name.as_str(), &h.value.0);
        }
        let response = send(&address, Scheme::Http, &wire)
            .await
            .map_err(|_| HttpError::Unreachable)?;
        Ok(HttpResponse {
            status: Status(response.status),
            headers: response
                .headers
                .iter()
                .map(|(n, v)| porter_http::Header::new(n, v.clone()))
                .collect(),
            body: response.body,
        })
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

#[tokio::test]
async fn the_device_flow_polls_until_the_person_approves() {
    let (issuer, endpoints) = issuer().await;
    let device = request_device_code(&LoopbackHttp, &endpoints, &client(), "Mail.Read")
        .await
        .expect("device code");
    assert!(device.user_code.starts_with("FAKE-") && device.interval == 1);
    // The person approves while the second wait; the first poll sees `pending`.
    let waits = Arc::new(Mutex::new(Vec::new()));
    let sleep = {
        let (waits, handle, user_code) =
            (waits.clone(), (*issuer).clone(), device.user_code.clone());
        move |seconds: u32| {
            let mut log = waits.lock().expect("lock");
            log.push(seconds);
            if log.len() == 2 {
                handle.approve_device(&user_code);
            }
            std::future::ready(())
        }
    };
    let tokens = await_device(&LoopbackHttp, &endpoints, &client(), &device, sleep)
        .await
        .expect("tokens");
    assert!(issuer.access_is_live(tokens.access_token.expose()));
    assert_eq!(*waits.lock().expect("lock"), vec![1, 1]);
}

#[tokio::test]
async fn the_device_flow_expires_when_nobody_answers_and_stops_when_they_decline() {
    let (issuer, endpoints) = issuer().await;
    let mut device = request_device_code(&LoopbackHttp, &endpoints, &client(), "Mail.Read")
        .await
        .expect("device code");
    device.expires_in = 5;
    let nothing = |_: u32| std::future::ready(());
    assert_eq!(
        await_device(&LoopbackHttp, &endpoints, &client(), &device, nothing).await,
        Err(DeviceFault::Expired)
    );

    issuer.deny_device(&device.user_code);
    device.expires_in = 600;
    assert_eq!(
        await_device(&LoopbackHttp, &endpoints, &client(), &device, nothing).await,
        Err(DeviceFault::Denied)
    );
}

#[tokio::test]
async fn an_issuer_without_a_device_endpoint_says_so() {
    let (_issuer, endpoints) = issuer().await;
    let none = IssuerEndpoints {
        device: None,
        ..endpoints
    };
    assert_eq!(
        request_device_code(&LoopbackHttp, &none, &client(), "x")
            .await
            .err(),
        Some(DeviceFault::Unsupported)
    );
}

/// A stand-in for OpenRouter's mint: `POST /api/v1/auth/keys` with the code and verifier.
async fn mint_server() -> (FakeAddress, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = Listener::bind(&Bind::Loopback, "mint").await.expect("bind");
    let address = listener.address().clone();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let record = bodies.clone();
    tokio::spawn(serve(
        listener,
        None,
        Arc::new(move |request: Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
            record.lock().expect("lock").push(body.clone());
            let right = request.path() == "/api/v1/auth/keys" && body["code"] == "good-code";
            match right {
                true => Response::json(200, &serde_json::json!({ "key": "sk-or-minted" })),
                false => Response::json(403, &serde_json::json!({ "error": "bad code" })),
            }
        }),
    ));
    (address, bodies)
}

#[tokio::test]
async fn the_openrouter_mint_turns_a_code_into_an_api_key() {
    let (address, bodies) = mint_server().await;
    let FakeAddress::Loopback(port) = address else {
        unreachable!("loopback")
    };
    let mut endpoints = Issuer::OpenRouter.endpoints();
    endpoints.token =
        EndpointUrl::parse(&format!("http://127.0.0.1:{port}/api/v1/auth/keys")).expect("url");
    let pkce = pkce();
    let code = |text: &str| porter_oauth::AuthCode(SecretText::new(text));

    let key = mint_key(&LoopbackHttp, &endpoints, &pkce, &code("good-code")).await;
    assert_eq!(key, Ok(Credential::ApiKey(SecretText::new("sk-or-minted"))));
    let sent = bodies.lock().expect("lock")[0].clone();
    assert_eq!(sent["code_verifier"], pkce.verifier.expose());
    assert_eq!(sent["code_challenge_method"], "S256");

    let bad = mint_key(&LoopbackHttp, &endpoints, &pkce, &code("spent")).await;
    assert_eq!(bad, Err(ExchangeFault::Refused));
}

#[tokio::test]
async fn the_openrouter_page_url_carries_the_challenge_and_a_stateful_callback() {
    let server = LoopbackServer::bind().await.expect("bind");
    let pkce = pkce();
    let url = openrouter_auth_url(
        &Issuer::OpenRouter.endpoints(),
        &pkce,
        &server.redirect_uri(),
    );
    assert!(url.starts_with("https://openrouter.ai/auth?callback_url="));
    assert!(url.contains(&pkce.challenge().0));
    // The callback the page returns to is one this listener accepts with that state.
    let callback = format!(
        "http://127.0.0.1:{}/?state={}&code=c0",
        server.port(),
        pkce.state.0
    );
    let sender = tokio::spawn(async move {
        let (address, target) = split_loopback(&callback).expect("loopback");
        send(&address, Scheme::Http, &Request::new("GET", &target)).await
    });
    let code = server.wait(&pkce.state).await.expect("code");
    assert_eq!(code.0.expose(), "c0");
    sender.await.expect("task").expect("answer");
}
