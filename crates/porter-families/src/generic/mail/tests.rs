use super::*;

fn spec() -> ProviderSpec {
    porter_provider::parse_provider(include_str!("../../../../../providers/generic-imap.toml"))
        .expect("the shipped file")
}

#[test]
fn an_address_has_a_domain_only_when_it_is_one() {
    const CASES: &[(&str, Option<&str>)] = &[
        ("ada@example.org", Some("example.org")),
        ("Ada@Example.ORG", Some("example.org")),
        ("ada", None),
        ("@example.org", None),
        ("ada@", None),
        ("ada@exa mple.org", None),
    ];
    for (address, want) in CASES {
        assert_eq!(
            domain_of(address).as_ref().map(DomainName::as_str),
            *want,
            "{address}"
        );
    }
}

#[test]
fn a_typed_host_makes_implicit_tls_imap_and_starttls_submission_logged_in_as_the_address() {
    let (endpoints, claims) =
        typed(" mail.example.org ", "ada@example.org", &spec()).expect("endpoints");
    let shown: Vec<_> = endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.clone()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (
                Family::Imap,
                "imaps://mail.example.org:993".into(),
                Tls::Implicit,
                "ada@example.org".into()
            ),
            (
                Family::Smtp,
                "smtp://mail.example.org:587".into(),
                Tls::StartTls,
                "ada@example.org".into()
            ),
        ]
    );
    assert_eq!(claims.len(), 1, "the file's one mail row");
}

#[test]
fn a_typed_host_that_is_not_a_name_is_refused() {
    for bad in [
        "",
        "mail example.org",
        "https://mail.example.org",
        "mail_example.org",
    ] {
        assert_eq!(
            typed(bad, "ada@example.org", &spec()).err(),
            Some(SignInFault::Unreadable),
            "{bad:?}"
        );
    }
}
