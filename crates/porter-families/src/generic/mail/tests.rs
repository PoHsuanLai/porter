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

fn hop(host: &str, port: u16, security: Security) -> Hop {
    Hop {
        host: host.to_owned(),
        port,
        security,
    }
}

fn servers(incoming: Hop, outgoing: Hop, login: Option<&str>) -> MailServers {
    MailServers {
        incoming,
        outgoing,
        login: login.map(str::to_owned),
    }
}

fn shown(servers: &MailServers, address: &str) -> Vec<(Family, String, Tls, String)> {
    let (endpoints, claims) = typed(Family::Imap, servers, address, &spec()).expect("endpoints");
    assert_eq!(claims.len(), 1, "the file's one mail row");
    endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.clone()))
        .collect()
}

#[test]
fn typed_servers_make_imap_and_smtp_as_their_ports_and_securities_say() {
    let tls = servers(
        hop("mail.example.org", 993, Security::Tls),
        hop("smtp.example.org", 465, Security::Tls),
        None,
    );
    assert_eq!(
        shown(&tls, "ada@example.org"),
        vec![
            (
                Family::Imap,
                "imaps://mail.example.org:993".into(),
                Tls::Implicit,
                "ada@example.org".into()
            ),
            (
                Family::Smtp,
                "smtps://smtp.example.org:465".into(),
                Tls::Implicit,
                "ada@example.org".into()
            ),
        ]
    );
    let starttls = servers(
        hop("mail.example.org", 1143, Security::StartTls),
        hop("mail.example.org", 2587, Security::StartTls),
        None,
    );
    assert_eq!(
        shown(&starttls, "ada@example.org"),
        vec![
            (
                Family::Imap,
                "imap://mail.example.org:1143".into(),
                Tls::StartTls,
                "ada@example.org".into()
            ),
            (
                Family::Smtp,
                "smtp://mail.example.org:2587".into(),
                Tls::StartTls,
                "ada@example.org".into()
            ),
        ]
    );
}

#[test]
fn a_typed_login_name_is_the_login_of_both_servers() {
    let named = servers(
        hop("mail.example.org", 993, Security::Tls),
        hop("smtp.example.org", 587, Security::StartTls),
        Some("ada.l"),
    );
    let logins: Vec<_> = shown(&named, "ada@example.org")
        .into_iter()
        .map(|(_, _, _, login)| login)
        .collect();
    assert_eq!(logins, ["ada.l", "ada.l"]);
}

#[test]
fn plain_is_for_loopback_only() {
    let local = servers(
        hop("127.0.0.1", 143, Security::Plain),
        hop("127.0.0.1", 587, Security::Plain),
        None,
    );
    let tls: Vec<_> = shown(&local, "ada@example.org")
        .into_iter()
        .map(|(_, _, tls, _)| tls)
        .collect();
    assert_eq!(tls, [Tls::Plain, Tls::Plain]);
    let elsewhere = servers(
        hop("mail.example.org", 143, Security::Plain),
        hop("smtp.example.org", 587, Security::StartTls),
        None,
    );
    assert_eq!(
        typed(Family::Imap, &elsewhere, "ada@example.org", &spec()).err(),
        Some(SignInFault::Unreadable)
    );
}

#[test]
fn a_pop3_account_claims_a_pop3_mailbox_and_an_imap_one_keeps_the_files_claim() {
    let same = servers(
        hop("pop.example.org", 995, Security::Tls),
        hop("smtp.example.org", 465, Security::Tls),
        None,
    );
    let transport = |family| {
        let (_, claims) = typed(family, &same, "ada@example.org", &spec()).expect("typed");
        claims
            .iter()
            .find_map(|c| match &c.offer {
                Offer::Present(Capability::Mail(mail)) => Some(mail.transport),
                _ => None,
            })
            .expect("a mail claim")
    };
    assert_eq!(transport(Family::Pop3), MailTransport::Pop3);
    assert_eq!(transport(Family::Imap), MailTransport::Imap);
}
