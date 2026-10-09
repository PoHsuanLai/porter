use super::*;

#[test]
fn a_started_flow_names_the_page_the_poll_endpoint_and_the_token() {
    let body = br#"{"poll":{"token":"t0k","endpoint":"https://cloud.example.org/index.php/login/v2/poll"},"login":"https://cloud.example.org/index.php/login/v2/flow/abc"}"#;
    let got = started(body).expect("a flow");
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
        assert_eq!(started(body.as_bytes()), None, "{case}");
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
