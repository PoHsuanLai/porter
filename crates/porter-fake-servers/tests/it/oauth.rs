//! The fake issuer driven with a minimal client and the scripted browser.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use porter_fake::{FakeAddress, FakeServer};
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Response, Scheme, post_form, send, serve};
use porter_fake_servers::net::{Bind, Listener};
use porter_fake_servers::oauth::{Consent, FakeIssuer, IssuerEvent, TokenResult};
use porter_fake_servers::seen::{Running, Seen};
use porter_fake_servers::{follow, shipped};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const VERIFIER: &str = "a-long-enough-code-verifier-for-the-fake-0123456789";

fn refresh_pairs(token: &str) -> [(&str, &str); 3] {
    [
        ("grant_type", "refresh_token"),
        ("refresh_token", token),
        ("client_id", "client-1"),
    ]
}

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))
}

/// The app's loopback redirect: records what the browser sent it.
async fn catcher() -> (FakeAddress, Seen<String>, tokio::task::JoinHandle<()>) {
    let listener = Listener::bind(&Bind::Loopback, "redirect")
        .await
        .expect("bind");
    let address = listener.address().clone();
    let seen = Seen::default();
    let record = seen.clone();
    let task = tokio::spawn(serve(
        listener,
        None,
        Arc::new(move |request: Request| {
            record.push(request.target.clone());
            Response::new(200).typed("text/html", "you may close this tab")
        }),
    ));
    (address, seen, task)
}

fn port(address: &FakeAddress) -> u16 {
    match address {
        FakeAddress::Loopback(p) => *p,
        FakeAddress::Socket(_) => unreachable!("loopback"),
    }
}

fn authorize_url(base: &str, redirect: &str, extra: &str) -> String {
    format!(
        "{base}/authorize?response_type=code&client_id=client-1&redirect_uri={redirect}&state=st-1\
         &scope=mail&code_challenge={}&code_challenge_method=S256{extra}",
        challenge()
    )
}

async fn token(address: &FakeAddress, pairs: &[(&str, &str)]) -> Response {
    post_form(address, "/token", pairs)
        .await
        .expect("token request")
}

/// Signs in through the browser and returns the code the app's redirect received.
async fn sign_in(issuer: &Running<porter_fake_servers::IssuerHandle>) -> (String, String) {
    let (redirect_addr, seen, _task) = catcher().await;
    let redirect = format!("http://127.0.0.1:{}/cb", port(&redirect_addr));
    let visit = follow(&authorize_url(issuer.base_url(), &redirect, ""))
        .await
        .expect("browser");
    assert_eq!(visit.landing.status, 200);
    let target = seen.all().remove(0);
    let code = target
        .split("code=")
        .nth(1)
        .and_then(|r| r.split('&').next())
        .expect("code")
        .to_owned();
    assert!(target.contains("state=st-1"), "state is echoed: {target}");
    (code, redirect)
}

#[tokio::test]
async fn pkce_sign_in_rotates_and_revokes() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let (address, _) = split_loopback(issuer.base_url()).expect("address");
    let (code, redirect) = sign_in(&issuer).await;

    let exchange = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("client_id", "client-1"),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", VERIFIER),
    ];
    let first = token(&address, &exchange).await.json_body().expect("json");
    let refresh1 = first["refresh_token"].as_str().expect("refresh").to_owned();
    let access1 = first["access_token"].as_str().expect("access").to_owned();
    assert_eq!(first["token_type"], "Bearer");
    assert!(issuer.access_is_live(&access1) && issuer.refresh_is_live(&refresh1));

    // The code is single use.
    let again = token(&address, &exchange).await;
    assert_eq!(
        (
            again.status,
            again.json_body().expect("json")["error"].clone()
        ),
        (400, "invalid_grant".into())
    );

    // Refresh rotates: the old token is spent.
    let second = token(&address, &refresh_pairs(&refresh1))
        .await
        .json_body()
        .expect("json");
    let refresh2 = second["refresh_token"]
        .as_str()
        .expect("rotated")
        .to_owned();
    assert_ne!(refresh1, refresh2);
    assert!(!issuer.refresh_is_live(&refresh1) && issuer.refresh_is_live(&refresh2));
    assert_eq!(token(&address, &refresh_pairs(&refresh1)).await.status, 400);

    // Revoke kills the live token; revoking an unknown one is still a 200.
    let revoked = post_form(
        &address,
        "/revoke",
        &[("token", &refresh2), ("client_id", "client-1")],
    )
    .await
    .expect("revoke");
    assert_eq!(revoked.status, 200);
    assert!(!issuer.refresh_is_live(&refresh2));
    assert_eq!(
        post_form(&address, "/revoke", &[("token", "nope")])
            .await
            .expect("revoke")
            .status,
        200
    );
    assert_eq!(token(&address, &refresh_pairs(&refresh2)).await.status, 400);

    let events = issuer.events();
    assert!(
        matches!(&events[0], IssuerEvent::Authorize { consent: Consent::Grant, state: Some(s), .. } if s == "st-1")
    );
    assert!(events.contains(&IssuerEvent::Revoke {
        token: refresh2,
        known: true
    }));
    assert!(events.iter().any(|e| matches!(e, IssuerEvent::Token { result: TokenResult::Rejected(r), .. } if r == "invalid_grant")));
}

#[tokio::test]
async fn a_wrong_verifier_or_redirect_is_invalid_grant() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let (address, _) = split_loopback(issuer.base_url()).expect("address");
    for (verifier, redirect_override) in [
        ("wrong-verifier", None),
        (VERIFIER, Some("http://127.0.0.1:1/other")),
    ] {
        let (code, redirect) = sign_in(&issuer).await;
        let redirect = redirect_override.map_or(redirect, str::to_owned);
        let reply = token(
            &address,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", "client-1"),
                ("redirect_uri", &redirect),
                ("code_verifier", verifier),
            ],
        )
        .await;
        assert_eq!(reply.json_body().expect("json")["error"], "invalid_grant");
    }
}

#[tokio::test]
async fn authorize_without_s256_is_refused_and_denial_redirects_with_an_error() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let (redirect_addr, seen, _task) = catcher().await;
    let redirect = format!("http://127.0.0.1:{}/cb", port(&redirect_addr));

    let plain = format!(
        "{}/authorize?response_type=code&client_id=c&redirect_uri={redirect}&state=s&code_challenge=x&code_challenge_method=plain",
        issuer.base_url()
    );
    follow(&plain).await.expect("browser");
    assert!(seen.all()[0].contains("error=invalid_request"));
    assert!(matches!(
        issuer.events()[0],
        IssuerEvent::AuthorizeRefused { .. }
    ));

    issuer.set_consent(Consent::Deny);
    follow(&authorize_url(issuer.base_url(), &redirect, ""))
        .await
        .expect("browser");
    assert!(seen.all()[1].contains("error=access_denied") && seen.all()[1].contains("state=st-1"));
}

#[tokio::test]
async fn refresh_can_be_refused_and_a_seeded_token_works() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let (address, _) = split_loopback(issuer.base_url()).expect("address");
    let seeded = issuer.seed_refresh("client-1", "mail");
    issuer.refuse_refreshes(1);
    let pairs = [
        ("grant_type", "refresh_token"),
        ("refresh_token", seeded.as_str()),
        ("client_id", "client-1"),
    ];
    assert_eq!(
        token(&address, &pairs).await.json_body().expect("json")["error"],
        "invalid_grant"
    );
    // The refusal did not spend the token: the next try works.
    assert_eq!(token(&address, &pairs).await.status, 200);
    // Another client cannot use it.
    let other = issuer.seed_refresh("client-1", "mail");
    let stolen = [
        ("grant_type", "refresh_token"),
        ("refresh_token", other.as_str()),
        ("client_id", "client-2"),
    ];
    assert_eq!(token(&address, &stolen).await.status, 400);
}

#[tokio::test]
async fn device_code_flow_pends_then_approves_or_denies() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let (address, _) = split_loopback(issuer.base_url()).expect("address");
    let start = |scope: &'static str| {
        let address = address.clone();
        async move {
            post_form(
                &address,
                "/device",
                &[("client_id", "client-1"), ("scope", scope)],
            )
            .await
            .expect("device")
            .json_body()
            .expect("json")
        }
    };
    let poll = |code: String| {
        let address = address.clone();
        async move {
            token(
                &address,
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", &code),
                    ("client_id", "client-1"),
                ],
            )
            .await
        }
    };

    let a = start("mail").await;
    let (code_a, user_a) = (
        a["device_code"].as_str().expect("code").to_owned(),
        a["user_code"].as_str().expect("user").to_owned(),
    );
    assert_eq!(
        poll(code_a.clone()).await.json_body().expect("json")["error"],
        "authorization_pending"
    );
    issuer.approve_device(&user_a);
    let done = poll(code_a.clone()).await;
    assert_eq!(done.status, 200);
    assert!(
        done.json_body().expect("json")["refresh_token"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        poll(code_a).await.json_body().expect("json")["error"],
        "invalid_grant"
    );

    let b = start("mail").await;
    issuer.deny_device(b["user_code"].as_str().expect("user"));
    assert_eq!(
        poll(b["device_code"].as_str().expect("code").to_owned())
            .await
            .json_body()
            .expect("json")["error"],
        "access_denied"
    );
}

#[tokio::test]
async fn unknown_paths_404_and_endpoints_point_at_the_issuer() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let (address, _) = split_loopback(issuer.base_url()).expect("address");
    let reply = send(&address, Scheme::Http, &Request::new("GET", "/nope"))
        .await
        .expect("get");
    assert_eq!(reply.status, 404);
    let endpoints = issuer.endpoints();
    assert_eq!(
        endpoints.token.as_str(),
        format!("{}/token", issuer.base_url())
    );
    assert!(endpoints.revoke.is_some() && endpoints.device.is_some());
}

#[tokio::test]
async fn the_issuer_leaves_the_google_file_as_shipped() {
    let fake = FakeIssuer::bind().await.expect("issuer");
    let google = shipped::google();
    assert_eq!(fake.rewrite(&google), google);
}
