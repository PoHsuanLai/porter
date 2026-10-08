//! A Graph drive that accepts the access tokens the fake issuer minted, and no other.

use porter_fake::FakeAddress;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Scheme, post_form, send};
use porter_fake_servers::oauth::TokenResult;
use porter_fake_servers::{FakeGraph, FakeIssuer, IssuerEvent};

async fn status(address: &FakeAddress, bearer: Option<&str>) -> u16 {
    let mut request = Request::new("GET", "/v1.0/me/drive");
    if let Some(token) = bearer {
        request = request.with_header("Authorization", &format!("Bearer {token}"));
    }
    send(address, Scheme::Http, &request)
        .await
        .expect("request")
        .status
}

#[tokio::test]
async fn the_drive_serves_an_access_token_the_issuer_minted_until_it_is_revoked() {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let graph = FakeGraph::start_issued(&issuer).await.expect("graph");
    let (drive, _) = split_loopback(graph.base_url()).expect("address");
    let (oauth, _) = split_loopback(issuer.base_url()).expect("address");

    let refresh = issuer.seed_refresh("client-1", "files");
    post_form(
        &oauth,
        "/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", "client-1"),
        ],
    )
    .await
    .expect("token");
    let access = issuer
        .events()
        .into_iter()
        .find_map(|event| match event {
            IssuerEvent::Token {
                result: TokenResult::Issued { access, .. },
                ..
            } => Some(access),
            _ => None,
        })
        .expect("a token was issued");

    assert_eq!(status(&drive, Some(&access)).await, 200, "a minted token");
    assert_eq!(
        status(&drive, Some("fake-access-999")).await,
        401,
        "unknown"
    );
    assert_eq!(status(&drive, None).await, 401, "no bearer");

    post_form(&oauth, "/revoke", &[("token", &access)])
        .await
        .expect("revoke");
    assert_eq!(status(&drive, Some(&access)).await, 401, "a revoked token");
}

#[tokio::test]
async fn a_drive_bound_to_one_token_still_serves_only_that_token() {
    let graph = FakeGraph::start("the-token").await.expect("graph");
    let (drive, _) = split_loopback(graph.base_url()).expect("address");
    assert_eq!(status(&drive, Some("the-token")).await, 200);
    assert_eq!(status(&drive, Some("fake-access-1")).await, 401);
}

#[tokio::test]
async fn me_says_who_the_token_is_for_and_only_to_a_token_the_drive_accepts() {
    let graph = FakeGraph::start("tok-1").await.expect("graph");
    let (drive, _) = split_loopback(graph.base_url()).expect("address");
    let me = |bearer: Option<&str>| {
        let mut request = Request::new("GET", "/v1.0/me");
        if let Some(token) = bearer {
            request = request.with_header("Authorization", &format!("Bearer {token}"));
        }
        let drive = drive.clone();
        async move { send(&drive, Scheme::Http, &request).await.expect("request") }
    };

    assert_eq!(me(None).await.status, 401);
    assert_eq!(me(Some("tok-2")).await.status, 401);
    let answer = me(Some("tok-1")).await;
    assert_eq!(answer.status, 200);
    let who: serde_json::Value = serde_json::from_slice(&answer.body).expect("json");
    // The two fields the Microsoft sign-in reads: `mail` first, then `userPrincipalName`.
    assert_eq!(who["mail"], porter_fake_servers::graph::DEFAULT_MAIL);
    assert_eq!(who["userPrincipalName"], who["mail"]);

    graph.set_mail("grace@work.example");
    let who: serde_json::Value =
        serde_json::from_slice(&me(Some("tok-1")).await.body).expect("json");
    assert_eq!(who["mail"], "grace@work.example");
}
