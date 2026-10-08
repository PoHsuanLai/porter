use super::*;

fn url(text: &str) -> EndpointUrl {
    EndpointUrl::parse(text).expect("url")
}

#[test]
fn a_typed_server_becomes_an_https_url_and_plain_http_is_for_this_computer_only() {
    const CASES: &[(&str, &str, Option<&str>)] = &[
        (
            "a bare host",
            "cloud.example.org",
            Some("https://cloud.example.org"),
        ),
        (
            "spaces and a slash",
            "  cloud.example.org/ ",
            Some("https://cloud.example.org"),
        ),
        (
            "a port",
            "cloud.example.org:8443",
            Some("https://cloud.example.org:8443"),
        ),
        (
            "a sub-path",
            "https://example.org/nextcloud/",
            Some("https://example.org/nextcloud"),
        ),
        (
            "loopback over http",
            "http://127.0.0.1:8080",
            Some("http://127.0.0.1:8080"),
        ),
        ("http elsewhere", "http://cloud.example.org", None),
        ("another scheme", "imaps://mail.example.org", None),
        ("nothing", "   ", None),
        ("user info", "https://ada@cloud.example.org", None),
    ];
    for (case, typed, want) in CASES {
        assert_eq!(
            parse_server(typed)
                .map(|u| u.as_str().to_owned())
                .as_deref(),
            *want,
            "{case}"
        );
    }
}

#[test]
fn paths_join_and_hrefs_resolve_on_the_servers_own_origin() {
    let base = url("https://example.org/nextcloud");
    assert_eq!(
        join(&base, "/index.php/login/v2")
            .map(|u| u.to_string())
            .as_deref(),
        Some("https://example.org/nextcloud/index.php/login/v2")
    );
    const CASES: &[(&str, &str, Option<&str>)] = &[
        (
            "a path",
            "/remote.php/dav/principals/users/ada/",
            Some("https://example.org/remote.php/dav/principals/users/ada/"),
        ),
        (
            "a url of the origin",
            "https://example.org/dav/",
            Some("https://example.org/dav/"),
        ),
        ("a url of another origin", "https://evil.example/dav/", None),
        ("a relative href", "dav/", None),
    ];
    for (case, href, want) in CASES {
        assert_eq!(
            resolve(&base, href).map(|u| u.to_string()).as_deref(),
            *want,
            "{case}"
        );
    }
    let with_port = url("http://127.0.0.1:8080/sub");
    assert_eq!(
        resolve(&with_port, "/dav/")
            .map(|u| u.to_string())
            .as_deref(),
        Some("http://127.0.0.1:8080/dav/")
    );
}

#[test]
fn a_user_id_is_a_url_segment_only_once_encoded() {
    assert_eq!(segment("alice"), "alice");
    assert_eq!(segment("ada@example.org"), "ada%40example.org");
    assert_eq!(segment("a b"), "a%20b");
}

#[test]
fn basic_encodes_the_login_and_the_password_together() {
    assert_eq!(
        basic("alice", &SecretText::new("secret")),
        "Basic YWxpY2U6c2VjcmV0"
    );
}

#[test]
fn what_was_typed_is_read_by_field_and_a_password_keeps_its_spaces() {
    let answers = vec![
        FieldAnswer {
            kind: FieldKind::Address,
            value: FieldValue::Plain("  ada@example.org ".into()),
        },
        FieldAnswer {
            kind: FieldKind::Password,
            value: FieldValue::Secret(SecretText::new(" pw ")),
        },
        FieldAnswer {
            kind: FieldKind::Server,
            value: FieldValue::Plain("   ".into()),
        },
    ];
    assert_eq!(
        text_of(&answers, FieldKind::Address).as_deref(),
        Some("ada@example.org")
    );
    assert_eq!(
        secret_of(&answers, FieldKind::Password)
            .as_ref()
            .map(SecretText::expose),
        Some(" pw ")
    );
    assert_eq!(
        text_of(&answers, FieldKind::Server),
        None,
        "blank is nothing"
    );
    assert_eq!(text_of(&answers, FieldKind::Username), None);
}

/// ship-5: the password is read from the app password field, or from a password field (a host
/// may answer an app password form with either kind).
#[cfg(feature = "generic")]
#[test]
fn the_typed_password_is_the_app_password_else_the_password() {
    let answer = |kind, text: &str| FieldAnswer {
        kind,
        value: FieldValue::Secret(SecretText::new(text)),
    };
    let read = |answers: &[FieldAnswer]| typed_password(answers).map(|s| s.expose().to_owned());
    assert_eq!(
        read(&[answer(FieldKind::AppPassword, "app")]).as_deref(),
        Some("app")
    );
    assert_eq!(
        read(&[answer(FieldKind::Password, "pw")]).as_deref(),
        Some("pw")
    );
    assert_eq!(
        read(&[
            answer(FieldKind::Password, "pw"),
            answer(FieldKind::AppPassword, "app")
        ])
        .as_deref(),
        Some("app")
    );
    assert_eq!(read(&[answer(FieldKind::AppPassword, "")]), None);
}
