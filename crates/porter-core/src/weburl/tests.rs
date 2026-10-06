use super::*;

/// The canonical text, the path and the query of a URL that is accepted.
type Parsed = Option<(&'static str, &'static str, Option<&'static str>)>;

#[test]
fn what_a_web_url_is_and_what_it_is_not() {
    let cases: &[(&str, Parsed)] = &[
        // (text, Some((canonical text, path, query)) when accepted)
        (
            "https://login.example/a?x=1&y=2",
            Some(("https://login.example/a?x=1&y=2", "/a", Some("x=1&y=2"))),
        ),
        (
            "https://login.example",
            Some(("https://login.example", "/", None)),
        ),
        (
            "https://login.example/",
            Some(("https://login.example/", "/", None)),
        ),
        (
            "https://login.example?x=1",
            Some(("https://login.example/?x=1", "/", Some("x=1"))),
        ),
        (
            "https://h:8443/p/q?e=a%40b.test",
            Some((
                "https://h:8443/p/q?e=a%40b.test",
                "/p/q",
                Some("e=a%40b.test"),
            )),
        ),
        (
            "https://h/p?mail=a@b.test",
            Some(("https://h/p?mail=a@b.test", "/p", Some("mail=a@b.test"))),
        ),
        (
            "https://h/a@b/c?x=y@z",
            Some(("https://h/a@b/c?x=y@z", "/a@b/c", Some("x=y@z"))),
        ),
        ("https://h/p?", Some(("https://h/p?", "/p", Some("")))),
        (
            "http://127.0.0.1:5000/cb?code=1",
            Some(("http://127.0.0.1:5000/cb?code=1", "/cb", Some("code=1"))),
        ),
        (
            "http://localhost/x",
            Some(("http://localhost/x", "/x", None)),
        ),
        (
            "http://[::1]:9/x?a=b",
            Some(("http://[::1]:9/x?a=b", "/x", Some("a=b"))),
        ),
        ("http://example.org/x", None),
        ("http://127.0.0.1.example.org/x", None),
        ("https://user:pw@login.example/a", None),
        ("https://user@login.example/a", None),
        ("https://login.example/a#frag", None),
        ("https://login.example/a?x=1#frag", None),
        ("ftp://login.example/a", None),
        ("imaps://mail.example:993", None),
        ("login.example/a", None),
        ("", None),
        ("https://", None),
        ("https://:443/a", None),
        ("https://h:0/a", None),
        ("https://h:99999/a", None),
        ("https://h:port/a", None),
        ("https://h /a", None),
        ("https://h/a b", None),
        ("https://h/a\r\nX: y", None),
        ("https://h/é", None),
    ];
    for (text, expected) in cases {
        let parsed = WebUrl::parse(text);
        match expected {
            Some((canon, path, query)) => {
                let url = parsed.unwrap_or_else(|e| panic!("{text}: {e}"));
                assert_eq!(url.as_str(), *canon, "{text}");
                assert_eq!((url.path(), url.query()), (*path, *query), "{text}");
            }
            None => assert!(parsed.is_err(), "{text} should be refused"),
        }
    }
}

#[test]
fn a_url_over_the_length_cap_is_refused() {
    let long = format!("https://h/?q={}", "a".repeat(MAX_LEN));
    assert!(WebUrl::parse(&long).is_err());
    let fits = format!("https://h/?q={}", "a".repeat(MAX_LEN - 14));
    assert!(WebUrl::parse(&fits).is_ok());
}

#[test]
fn the_origin_is_lower_case_with_the_default_port() {
    let cases = [
        ("https://Login.Example/a?x=1", "https://login.example:443"),
        ("https://h:8443/", "https://h:8443"),
        ("http://127.0.0.1:5000/cb", "http://127.0.0.1:5000"),
        ("http://[::1]/x?y", "http://::1:80"),
        ("https://h/a@b?c@d", "https://h:443"),
    ];
    for (text, origin) in cases {
        let url = WebUrl::parse(text).expect(text);
        assert_eq!(url.origin().to_string(), origin, "{text}");
    }
}

#[test]
fn the_serde_form_is_the_text_and_a_bad_text_is_refused() {
    let url = WebUrl::parse("https://login.example/a?x=1").expect("url");
    let json = serde_json::to_string(&url).expect("json");
    assert_eq!(json, "\"https://login.example/a?x=1\"");
    assert_eq!(serde_json::from_str::<WebUrl>(&json).expect("back"), url);
    assert!(serde_json::from_str::<WebUrl>("\"http://example.org/\"").is_err());
}

#[test]
fn an_endpoint_url_converts_when_it_is_a_web_url() {
    let cases = [
        ("https://cloud.example/remote.php/dav/", true),
        ("http://127.0.0.1:8080/", true),
        ("http://cloud.example/", false),
        ("imaps://mail.example:993", false),
    ];
    for (text, ok) in cases {
        let endpoint = EndpointUrl::parse(text).expect(text);
        assert_eq!(WebUrl::try_from(&endpoint).is_ok(), ok, "{text}");
    }
}
