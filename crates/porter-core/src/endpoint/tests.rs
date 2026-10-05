use super::*;

fn url(text: &str) -> EndpointUrl {
    EndpointUrl::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn endpoint(family: Family, text: &str, tls: Tls) -> ServiceEndpoint {
    ServiceEndpoint {
        family,
        url: url(text),
        tls,
        login: LoginName("ada@example.org".into()),
    }
}

#[test]
fn urls_parse_to_their_origin() {
    const CASES: &[(&str, &str, UrlScheme, &str, u16, &str)] = &[
        (
            "default https",
            "https://cloud.example.org",
            UrlScheme::Https,
            "cloud.example.org",
            443,
            "/",
        ),
        (
            "path kept",
            "https://cloud.example.org/remote.php/dav/",
            UrlScheme::Https,
            "cloud.example.org",
            443,
            "/remote.php/dav/",
        ),
        (
            "explicit port",
            "imaps://IMAP.Example.org:1993",
            UrlScheme::Imaps,
            "imap.example.org",
            1993,
            "/",
        ),
        (
            "imap default",
            "imap://mail.example.org",
            UrlScheme::Imap,
            "mail.example.org",
            143,
            "/",
        ),
        (
            "submission default",
            "smtp://mail.example.org",
            UrlScheme::Smtp,
            "mail.example.org",
            587,
            "/",
        ),
        (
            "smtps default",
            "smtps://mail.example.org",
            UrlScheme::Smtps,
            "mail.example.org",
            465,
            "/",
        ),
        (
            "managesieve default",
            "sieve://mail.example.org",
            UrlScheme::Sieve,
            "mail.example.org",
            4190,
            "/",
        ),
        (
            "managesieve over tls",
            "sieves://mail.example.org:5190",
            UrlScheme::Sieves,
            "mail.example.org",
            5190,
            "/",
        ),
        (
            "ipv6",
            "http://[::1]:8080/x",
            UrlScheme::Http,
            "::1",
            8080,
            "/x",
        ),
    ];
    for (name, text, scheme, host, port, path) in CASES {
        let parsed = url(text);
        let origin = parsed.origin();
        assert_eq!(
            (
                origin.scheme,
                origin.host.as_str(),
                origin.port,
                parsed.path()
            ),
            (*scheme, *host, *port, *path),
            "{name}"
        );
    }
}

#[test]
fn text_that_is_not_an_endpoint_url_is_refused() {
    const CASES: &[(&str, &str)] = &[
        ("empty", ""),
        ("no scheme", "cloud.example.org"),
        ("other scheme", "ftp://cloud.example.org"),
        ("no host", "https:///x"),
        ("user info", "https://ada@cloud.example.org"),
        ("query", "https://cloud.example.org/?a=b"),
        ("fragment", "https://cloud.example.org/#x"),
        ("space", "https://cloud example.org"),
        ("port zero", "https://cloud.example.org:0"),
        ("port text", "https://cloud.example.org:https"),
        ("port too large", "https://cloud.example.org:65536"),
    ];
    for (name, text) in CASES {
        assert!(EndpointUrl::parse(text).is_err(), "{name}");
    }
}

#[test]
fn origins_compare_whole_hosts_not_suffixes() {
    let granted = url("https://cloud.example.org/remote.php/dav/").origin();
    assert_eq!(url("https://CLOUD.example.org:443/other").origin(), granted);
    assert_ne!(url("https://evil-cloud.example.org/").origin(), granted);
    assert_ne!(
        url("https://cloud.example.org.evil.test/").origin(),
        granted
    );
    assert_ne!(url("http://cloud.example.org/").origin(), granted);
}

#[test]
fn an_endpoint_must_be_coherent() {
    type Case = (
        &'static str,
        Family,
        &'static str,
        Tls,
        Result<(), EndpointFault>,
    );
    const CASES: &[Case] = &[
        (
            "imaps",
            Family::Imap,
            "imaps://mail.example.org",
            Tls::Implicit,
            Ok(()),
        ),
        (
            "imap starttls",
            Family::Imap,
            "imap://mail.example.org",
            Tls::StartTls,
            Ok(()),
        ),
        (
            "smtp starttls",
            Family::Smtp,
            "smtp://mail.example.org:587",
            Tls::StartTls,
            Ok(()),
        ),
        (
            "webdav https",
            Family::WebDav,
            "https://cloud.example.org/dav/",
            Tls::Implicit,
            Ok(()),
        ),
        (
            "imap family on an https url",
            Family::Imap,
            "https://mail.example.org",
            Tls::Implicit,
            Err(EndpointFault::SchemeMismatch),
        ),
        (
            "https url claims starttls",
            Family::WebDav,
            "https://cloud.example.org",
            Tls::StartTls,
            Err(EndpointFault::TlsMismatch),
        ),
        (
            "imaps url claims plain",
            Family::Imap,
            "imaps://mail.example.org",
            Tls::Plain,
            Err(EndpointFault::TlsMismatch),
        ),
        (
            "plain http off this computer",
            Family::WebDav,
            "http://cloud.example.org",
            Tls::Plain,
            Err(EndpointFault::PlainOffLoopback),
        ),
        (
            "plain http on loopback",
            Family::WebDav,
            "http://127.0.0.1:8080",
            Tls::Plain,
            Ok(()),
        ),
        (
            "plain imap on localhost",
            Family::Imap,
            "imap://localhost:1143",
            Tls::Plain,
            Ok(()),
        ),
    ];
    for (name, family, text, tls, expected) in CASES {
        assert_eq!(endpoint(*family, text, *tls).check(), *expected, "{name}");
    }
}

#[test]
fn an_endpoint_url_reads_back_from_its_text_only() {
    let parsed = url("https://cloud.example.org/dav/");
    let json = serde_json::to_string(&parsed).expect("json");
    assert_eq!(json, r#""https://cloud.example.org/dav/""#);
    assert_eq!(
        serde_json::from_str::<EndpointUrl>(&json).expect("url"),
        parsed
    );
    assert!(serde_json::from_str::<EndpointUrl>(r#""nope""#).is_err());
}

#[test]
fn a_relay_plan_never_shows_its_credential() {
    let plan = RelayPlan {
        endpoint: endpoint(Family::Imap, "imaps://mail.example.org", Tls::Implicit),
        kind: CapabilityKind::Mail,
        auth: RelayAuth::Password(crate::SecretText::new("hunter2")),
    };
    assert!(!format!("{plan:?}").contains("hunter2"));
}
