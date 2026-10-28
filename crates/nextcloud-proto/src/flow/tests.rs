use super::*;

#[test]
fn a_started_flow_names_the_page_the_poll_endpoint_and_the_token() {
    let body = br#"{"poll":{"token":"t0k","endpoint":"https://cloud.example.org/index.php/login/v2/poll"},"login":"https://cloud.example.org/index.php/login/v2/flow/abc"}"#;
    let got = started_body(body).expect("a flow");
    assert_eq!(got.token.expose(), "t0k");
    // The token is a secret: Debug of the flow does not show it.
    assert!(!format!("{got:?}").contains("t0k"), "{got:?}");
    assert_eq!(
        got.poll.as_str(),
        "https://cloud.example.org/index.php/login/v2/poll"
    );
    assert_eq!(
        got.login.as_str(),
        "https://cloud.example.org/index.php/login/v2/flow/abc"
    );
}

#[test]
fn a_started_flow_that_is_not_one_is_refused() {
    const CASES: &[(&str, &str)] = &[
        ("not json", "<html>"),
        (
            "no token",
            r#"{"poll":{"endpoint":"https://c.example/p"},"login":"https://c.example/l"}"#,
        ),
        (
            "an empty token",
            r#"{"poll":{"token":"","endpoint":"https://c.example/p"},"login":"https://c.example/l"}"#,
        ),
        (
            "no login page",
            r#"{"poll":{"token":"t","endpoint":"https://c.example/p"}}"#,
        ),
        (
            "a plain poll endpoint elsewhere",
            r#"{"poll":{"token":"t","endpoint":"http://c.example/p"},"login":"https://c.example/l"}"#,
        ),
        (
            "a login page that is not http",
            r#"{"poll":{"token":"t","endpoint":"https://c.example/p"},"login":"imaps://c.example/l"}"#,
        ),
    ];
    for (case, body) in CASES {
        assert_eq!(started_body(body.as_bytes()), None, "{case}");
    }
}

#[test]
fn a_granted_flow_gives_the_login_name_and_the_app_password() {
    let body =
        br#"{"server":"https://cloud.example.org","loginName":"ada","appPassword":"abc-def"}"#;
    let got = granted(body).expect("granted");
    assert_eq!(got.login, LoginName("ada".into()));
    assert_eq!(got.password.expose(), "abc-def");
    assert_eq!(
        got.server.map(|s| s.to_string()).as_deref(),
        Some("https://cloud.example.org")
    );
}

#[test]
fn a_grant_missing_a_piece_is_refused_and_a_bad_server_is_only_ignored() {
    assert_eq!(granted(br#"{"loginName":"ada"}"#), None);
    assert_eq!(granted(br#"{"loginName":"ada","appPassword":""}"#), None);
    let odd_server =
        br#"{"server":"http://elsewhere.example","loginName":"ada","appPassword":"p"}"#;
    assert_eq!(granted(odd_server).expect("granted").server, None);
}

fn reply(status: u16, body: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(body.as_bytes().to_vec())
        .unwrap()
}

#[test]
fn the_start_is_a_post_to_the_login_flow_and_the_poll_posts_the_token() {
    let server = EndpointUrl::parse("https://cloud.example.org/nextcloud").unwrap();
    let start = start_request(&server).unwrap();
    assert_eq!(start.method(), "POST");
    assert_eq!(
        start.uri(),
        "https://cloud.example.org/nextcloud/index.php/login/v2"
    );
    let body = br#"{"poll":{"token":"t0k","endpoint":"https://cloud.example.org/p"},"login":"https://cloud.example.org/l"}"#;
    let flow = started(&reply(200, std::str::from_utf8(body).unwrap())).unwrap();
    let poll = poll_request(&flow).unwrap();
    assert_eq!(poll.method(), "POST");
    assert_eq!(poll.body(), b"token=t0k");
    assert_eq!(
        poll.headers()[CONTENT_TYPE],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(started(&reply(404, "")), Err(Error::Unreadable));
}

#[test]
fn plain_http_to_another_computer_builds_no_request() {
    let server = EndpointUrl::parse("http://cloud.example.org").unwrap();
    assert_eq!(start_request(&server).unwrap_err(), Error::Insecure);
    let local = EndpointUrl::parse("http://127.0.0.1:8080").unwrap();
    assert!(start_request(&local).is_ok());
}

#[test]
fn a_poll_is_waiting_hiccuping_granted_or_over() {
    assert_eq!(polled(&reply(404, "")), Polled::Waiting);
    for status in [429, 500, 503] {
        assert_eq!(polled(&reply(status, "")), Polled::Hiccup, "{status}");
    }
    assert_eq!(polled(&reply(403, "")), Polled::Failed);
    assert_eq!(polled(&reply(200, "not json")), Polled::Failed);
    let granted = reply(
        200,
        r#"{"server":"https://c.example","loginName":"ada","appPassword":"p"}"#,
    );
    assert!(matches!(polled(&granted), Polled::Granted(g) if g.login.0 == "ada"));
}
