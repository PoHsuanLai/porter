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
