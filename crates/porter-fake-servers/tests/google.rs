//! The fake Google: an issuer that behaves as Google's does and account APIs that know the scopes
//! a token was issued for.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use porter_fake_servers::browser::split_loopback;
use porter_fake_servers::http::{Request, Response, Scheme, post_form, send};
use porter_fake_servers::{Google, Style, shipped};
use sha2::{Digest, Sha256};

const SECRET: &str = "GOCSPX-fake";
const CLIENT: &str = "client-1";
const CALENDAR: &str = "https://www.googleapis.com/auth/calendar";
const TASKS: &str = "https://www.googleapis.com/auth/tasks";

fn body(response: &Response) -> serde_json::Value {
    response.json_body().expect("JSON")
}

async fn get(base: &str, path: &str, bearer: Option<&str>) -> Response {
    let (address, _) = split_loopback(base).expect("address");
    let mut request = Request::new("GET", path);
    if let Some(token) = bearer {
        request = request.with_header("Authorization", &format!("Bearer {token}"));
    }
    send(&address, Scheme::Http, &request).await.expect("get")
}

async fn token(google: &Google, pairs: &[(&str, &str)]) -> Response {
    let (address, _) = split_loopback(google.issuer.base_url()).expect("address");
    post_form(&address, "/token", pairs).await.expect("token")
}

/// A refresh token the person is taken to have granted `scope`.
fn granted(google: &Google, scope: &str) -> String {
    google.issuer.seed_refresh(CLIENT, scope)
}

#[tokio::test]
async fn refreshing_returns_an_access_token_with_its_scopes_and_keeps_the_refresh_token_valid() {
    let google = Google::start(SECRET).await.expect("google");
    let refresh = granted(&google, &format!("openid {CALENDAR}"));
    let pairs = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.as_str()),
        ("client_id", CLIENT),
        ("client_secret", SECRET),
    ];
    for _ in 0..2 {
        let answer = token(&google, &pairs).await;
        assert_eq!(answer.status, 200);
        let json = body(&answer);
        assert!(
            json.get("refresh_token").is_none(),
            "Google does not rotate: {json}"
        );
        assert_eq!(json["scope"], format!("openid {CALENDAR}"));
        assert_eq!(json["expires_in"], 3600);
    }
    assert!(google.issuer.refresh_is_live(&refresh));
    // The same issuer in the rotating style spends the token it was given.
    google.issuer.set_style(Style::Rotating);
    let rotated = token(&google, &pairs).await;
    assert!(body(&rotated).get("refresh_token").is_some());
    assert!(!google.issuer.refresh_is_live(&refresh));
}

#[tokio::test]
async fn the_application_secret_is_required_at_the_token_endpoint() {
    let google = Google::start(SECRET).await.expect("google");
    let refresh = granted(&google, CALENDAR);
    for secret in [None, Some("wrong")] {
        let mut pairs = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", CLIENT),
        ];
        pairs.extend(secret.map(|s| ("client_secret", s)));
        let answer = token(&google, &pairs).await;
        assert_eq!(answer.status, 400);
        assert_eq!(body(&answer)["error"], "invalid_client");
    }
    assert!(google.issuer.refresh_is_live(&refresh));
}

#[tokio::test]
async fn the_apis_admit_issued_tokens_for_the_scopes_they_carry_only() {
    let google = Google::start(SECRET).await.expect("google");
    let refresh = granted(&google, &format!("openid {CALENDAR}"));
    let access = body(
        &token(
            &google,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", CLIENT),
                ("client_secret", SECRET),
            ],
        )
        .await,
    )["access_token"]
        .as_str()
        .expect("access token")
        .to_owned();
    let api = google.api.base_url();
    let status = |path: &'static str, bearer: Option<&str>| {
        let bearer = bearer.map(str::to_owned);
        async move { get(api, path, bearer.as_deref()).await.status }
    };
    assert_eq!(
        status("/calendar/v3/users/me/calendarList", Some(&access)).await,
        200
    );
    assert_eq!(
        status("/v1/userinfo", Some(&access)).await,
        200,
        "openid is enough"
    );
    assert_eq!(
        status("/tasks/v1/users/@me/lists", Some(&access)).await,
        403,
        "{TASKS} was not granted"
    );
    assert_eq!(status("/v1/contactGroups", Some(&access)).await, 403);
    assert_eq!(
        status("/calendar/v3/users/me/calendarList", None).await,
        401
    );
    assert_eq!(
        status("/calendar/v3/users/me/calendarList", Some("not-a-token")).await,
        401
    );
    assert_eq!(status("/nothing", Some(&access)).await, 404);

    // Revoking the refresh token does not end the access token (Google's does end both; the
    // fake's revoke removes what it was given, so revoke the access token).
    let (issuer, _) = split_loopback(google.issuer.base_url()).expect("address");
    post_form(&issuer, "/revoke", &[("token", &access)])
        .await
        .expect("revoke");
    assert_eq!(
        status("/calendar/v3/users/me/calendarList", Some(&access)).await,
        401
    );
}

#[tokio::test]
async fn userinfo_names_the_user_and_the_knobs_change_answers() {
    let google = Google::start(SECRET).await.expect("google");
    let access = "planted-access";
    google.issuer.seed_access_as(access, CLIENT);
    let api = google.api.base_url();
    let me = body(&get(api, "/v1/userinfo", Some(access)).await);
    assert_eq!(me["email"], "ada@gmail.com");
    google.api.set_user("grace@firm.example", "Grace");
    let me = body(&get(api, "/v1/userinfo", Some(access)).await);
    assert_eq!(
        (me["email"].as_str(), me["name"].as_str()),
        (Some("grace@firm.example"), Some("Grace"))
    );
    google.api.refuse("/tasks/v1/users/@me/lists", 404);
    assert_eq!(
        get(api, "/tasks/v1/users/@me/lists", Some(access))
            .await
            .status,
        404
    );
    google.api.disable_api("/v1/contactGroups");
    let off = get(api, "/v1/contactGroups", Some(access)).await;
    assert_eq!(off.status, 403);
    assert!(off.text().contains("accessNotConfigured"));
    assert_eq!(google.api.hits().len(), 4);
}

#[tokio::test]
async fn only_a_code_that_asked_for_offline_access_gets_a_refresh_token() {
    let google = Google::start(SECRET).await.expect("google");
    let (issuer, _) = split_loopback(google.issuer.base_url()).expect("address");
    let verifier = "v".repeat(43);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    for (offline, want_refresh) in [(true, true), (false, false)] {
        let mut target = format!(
            "/authorize?response_type=code&client_id={CLIENT}&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2F\
             &state=s&code_challenge={challenge}&code_challenge_method=S256&scope=openid"
        );
        if offline {
            target.push_str("&access_type=offline&prompt=consent");
        }
        let page = send(&issuer, Scheme::Http, &Request::new("GET", &target))
            .await
            .expect("authorize");
        let location = page.header("location").expect("a redirect").to_owned();
        let code = location
            .split(['?', '&'])
            .find_map(|part| part.strip_prefix("code="))
            .expect("a code")
            .to_owned();
        let answer = token(
            &google,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("code_verifier", &verifier),
                ("redirect_uri", "http://127.0.0.1:9/"),
                ("client_id", CLIENT),
                ("client_secret", SECRET),
            ],
        )
        .await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        let has = body(&answer).get("refresh_token").is_some();
        assert_eq!(has, want_refresh, "offline={offline}");
    }
    let queries = google.issuer.authorize_queries();
    assert_eq!(queries.len(), 2);
    assert!(
        queries[0]
            .iter()
            .any(|(k, v)| k == "access_type" && v == "offline")
    );
}

#[tokio::test]
async fn the_shipped_file_keeps_its_paths_when_it_is_pointed_at_the_fake() {
    let google = Google::start(SECRET).await.expect("google");
    let spec = google.api.rewrite(&shipped::google());
    let rows: Vec<(String, String)> = spec
        .capabilities
        .iter()
        .filter_map(|row| {
            row.endpoint
                .as_ref()
                .map(|e| (row.family.slug(), e.0.clone()))
        })
        .collect();
    let base = google.api.base_url();
    assert!(
        rows.contains(&("google_calendar".into(), format!("{base}/calendar/v3"))),
        "{rows:?}"
    );
    assert!(rows.contains(&("google_tasks".into(), format!("{base}/tasks/v1"))));
    // Gmail's IMAP row is not an API of this server.
    assert!(rows.contains(&("imap".into(), "imaps://imap.gmail.com:993".into())));
}
