//! The Generic family against the fake autoconfig, DNS and DAV servers: a mail account found from
//! its address (and from a typed server when nothing is published), a DAV account found from its
//! server's name or its address, a refused password, and the session.
#![cfg(feature = "generic")]

use crate::common;

use common::mail::{MailWorld, Wire};
use common::{Fakes, Place, all_on, drive, plain, secret};
use porter_core::capability::CapabilityKind;
use porter_core::sheet::{FieldAnswer, FieldKind, SignInFault, SignInInput};
use porter_core::{AccountId, Credential, Family, Offer, SecretPurpose, SecretText, Tls};
use porter_discover::{MxRecord, SrvRecord};
use porter_fake::FakeServer;
use porter_fake_servers::net::Bind;
use porter_fake_servers::{
    Accounts, AutoconfigHandle, DavHandle, FakeAutoconfig, FakeDav, FakeDns, FakeImap, FakeSmtp,
    Running, autoconfig_xml, mailbox,
};
use porter_families::GenericProvider;
use porter_http::SharedHttp;
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSpec, SignInMode, SignInStart,
    SignInStep, Signed, parse_provider,
};

fn spec(text: &str) -> ProviderSpec {
    parse_provider(text).expect("the shipped file")
}

fn imap_spec() -> ProviderSpec {
    spec(include_str!("../../../../providers/generic-imap.toml"))
}

fn dav_spec() -> ProviderSpec {
    spec(include_str!("../../../../providers/generic-dav.toml"))
}

fn add() -> SignInStart {
    SignInStart::new(SignInMode::Add)
}

fn port_of(base: &str) -> u16 {
    base.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("port")
}

struct Mail {
    autoconfig: Running<AutoconfigHandle>,
    provider: GenericProvider,
    _servers: MailWorld,
}

/// The password the fake mail servers have planted.
const PASSWORD: &str = "s3cret";

async fn mail_with(dns: FakeDns) -> Mail {
    mail_as("ada@fake.test", dns).await
}

/// A mail world whose servers let `user` in with [`PASSWORD`].
async fn mail_as(user: &str, dns: FakeDns) -> Mail {
    let servers = MailWorld::start(user, PASSWORD).await;
    let autoconfig = FakeAutoconfig::start().await.expect("fake");
    let port = port_of(autoconfig.base_url());
    let places = ["autoconfig.fake.test", "fake.test"]
        .into_iter()
        .map(|host| Place {
            host,
            prefix: "/",
            port,
        })
        .collect();
    Mail {
        autoconfig,
        provider: GenericProvider::new(imap_spec(), SharedHttp::new(Fakes::new(places)), dns)
            .with_connect(servers.wire()),
        _servers: servers,
    }
}

/// Answers the first form with `address` and `password`, the question for a server with
/// `server`, and the review with everything on.
fn mail_person<'a>(
    address: &'a str,
    password: &'a str,
    server: &'a str,
) -> impl FnMut(&SignInStep) -> SignInInput + 'a {
    move |step| match step {
        SignInStep::AskFields(fields) if fields.iter().any(|f| f.kind == FieldKind::Address) => {
            SignInInput::Fields(vec![
                plain(FieldKind::Address, address),
                secret(FieldKind::Password, password),
            ])
        }
        SignInStep::AskFields(_) => SignInInput::Fields(vec![
            plain(FieldKind::Protocol, "imap"),
            plain(FieldKind::Server, server),
            plain(FieldKind::Security, "tls"),
            plain(FieldKind::OutgoingServer, server),
            plain(FieldKind::OutgoingSecurity, "starttls"),
        ]),
        SignInStep::Review { claims, .. } => {
            SignInInput::Confirm(all_on(claims.iter().map(|c| c.offer.kind())))
        }
        _ => SignInInput::Cancel,
    }
}

fn done(steps: &[SignInStep]) -> &Signed {
    match steps.last() {
        Some(SignInStep::Done(signed)) => signed,
        other => panic!("expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn a_mail_account_is_found_from_its_address_by_autoconfig() {
    let mail = mail_with(FakeDns::new()).await;
    mail.autoconfig.serve_autoconfig(&autoconfig_xml(
        "fake.test",
        ("imap.fake.test", 993),
        ("smtp.fake.test", 587),
    ));
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", "s3cret", "")).await;

    let SignInStep::AskFields(form) = &steps[0] else {
        panic!("a form first: {steps:?}");
    };
    let asked: Vec<FieldKind> = form.iter().map(|f| f.kind).collect();
    assert_eq!(asked, [FieldKind::Address, FieldKind::Password]);
    assert!(matches!(steps[1], SignInStep::Review { .. }), "{steps:?}");

    let signed = done(&steps);
    assert_eq!(signed.label.0, "ada@fake.test");
    let endpoints: Vec<_> = signed
        .endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.as_str()))
        .collect();
    assert_eq!(
        endpoints,
        vec![
            (
                Family::Imap,
                "imaps://imap.fake.test:993".to_owned(),
                Tls::Implicit,
                "ada@fake.test"
            ),
            (
                Family::Smtp,
                "smtp://smtp.fake.test:587".to_owned(),
                Tls::StartTls,
                "ada@fake.test"
            ),
        ]
    );
    assert!(
        signed
            .claims
            .iter()
            .any(|c| c.offer.kind() == CapabilityKind::Mail)
    );
    let [(SecretPurpose::Password, Credential::Password(password))] = signed.credentials.as_slice()
    else {
        panic!("one password: {:?}", signed.credentials);
    };
    assert_eq!(password.expose(), "s3cret");
}

#[tokio::test]
async fn srv_records_find_the_servers_when_no_document_is_published() {
    let dns = FakeDns::new()
        .with_srv(
            "_imaps._tcp.fake.test",
            vec![SrvRecord {
                priority: 0,
                weight: 1,
                port: 993,
                target: porter_provider::DomainName::parse("imap.fake.test").expect("name"),
            }],
        )
        .with_srv(
            "_submission._tcp.fake.test",
            vec![SrvRecord {
                priority: 0,
                weight: 1,
                port: 587,
                target: porter_provider::DomainName::parse("smtp.fake.test").expect("name"),
            }],
        );
    let mail = mail_with(dns).await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", PASSWORD, "")).await;
    let signed = done(&steps);
    assert_eq!(signed.endpoints.len(), 2, "{:?}", signed.endpoints);
    assert_eq!(
        signed.endpoints[0].url.to_string(),
        "imaps://imap.fake.test:993"
    );
}

#[tokio::test]
async fn a_domain_that_publishes_nothing_gets_a_question_for_the_server() {
    let dns = FakeDns::new().with_mx(
        "fake.test",
        vec![MxRecord {
            preference: 10,
            host: porter_provider::DomainName::parse("mx.fake.test").expect("name"),
        }],
    );
    let mail = mail_with(dns).await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        mail_person("ada@fake.test", PASSWORD, "mail.fake.test"),
    )
    .await;

    let questions: Vec<Vec<FieldKind>> = steps
        .iter()
        .filter_map(|s| match s {
            SignInStep::AskFields(f) => Some(f.iter().map(|f| f.kind).collect()),
            _ => None,
        })
        .collect();
    use FieldKind::*;
    assert_eq!(
        questions,
        vec![
            vec![Address, Password],
            vec![
                Protocol,
                Server,
                Security,
                Port,
                OutgoingServer,
                OutgoingSecurity,
                OutgoingPort,
                Username
            ]
        ]
    );
    let signed = done(&steps);
    let urls: Vec<String> = signed.endpoints.iter().map(|e| e.url.to_string()).collect();
    assert_eq!(
        urls,
        ["imaps://mail.fake.test:993", "smtp://mail.fake.test:587"]
    );
    assert_eq!(signed.label.0, "ada@fake.test");
}

/// The first form's answers, then `typed` for the question of the server form that a domain
/// publishing nothing leads to; the review with everything on.
fn manual_person<'a>(typed: Vec<FieldAnswer>) -> impl FnMut(&SignInStep) -> SignInInput + 'a {
    move |step| match step {
        SignInStep::AskFields(fields) if fields.iter().any(|f| f.kind == FieldKind::Address) => {
            SignInInput::Fields(vec![
                plain(FieldKind::Address, "ada@fake.test"),
                secret(FieldKind::Password, "s3cret"),
            ])
        }
        SignInStep::AskFields(_) => SignInInput::Fields(typed.clone()),
        SignInStep::Review { claims, .. } => {
            SignInInput::Confirm(all_on(claims.iter().map(|c| c.offer.kind())))
        }
        _ => SignInInput::Cancel,
    }
}

async fn nothing_published() -> Mail {
    mail_with(FakeDns::new()).await
}

fn shown(signed: &Signed) -> Vec<(Family, String, Tls, String)> {
    signed
        .endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.clone()))
        .collect()
}

#[tokio::test]
async fn typed_imap_and_smtp_on_other_ports_with_starttls_and_a_login_name() {
    let mail = mail_as("ada.login", FakeDns::new()).await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let typed = vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, "mail.fake.test"),
        plain(FieldKind::Security, "starttls"),
        plain(FieldKind::Port, "1143"),
        plain(FieldKind::OutgoingServer, "relay.fake.test"),
        plain(FieldKind::OutgoingSecurity, "starttls"),
        plain(FieldKind::OutgoingPort, "2587"),
        plain(FieldKind::Username, "ada.login"),
    ];
    let steps = drive(&mut signin, manual_person(typed)).await;
    let signed = done(&steps);
    assert_eq!(
        shown(signed),
        vec![
            (
                Family::Imap,
                "imap://mail.fake.test:1143".into(),
                Tls::StartTls,
                "ada.login".into()
            ),
            (
                Family::Smtp,
                "smtp://relay.fake.test:2587".into(),
                Tls::StartTls,
                "ada.login".into()
            ),
        ]
    );
    assert_eq!(
        signed.label.0, "ada@fake.test",
        "the account is the address"
    );
}

#[tokio::test]
async fn typed_ports_default_from_the_security_when_left_empty() {
    let mail = nothing_published().await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let typed = vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, "mail.fake.test"),
        plain(FieldKind::Security, "starttls"),
        plain(FieldKind::OutgoingServer, "mail.fake.test"),
        plain(FieldKind::OutgoingSecurity, "tls"),
    ];
    let steps = drive(&mut signin, manual_person(typed)).await;
    let urls: Vec<_> = done(&steps)
        .endpoints
        .iter()
        .map(|e| e.url.to_string())
        .collect();
    assert_eq!(
        urls,
        ["imap://mail.fake.test:143", "smtps://mail.fake.test:465"]
    );
}

#[tokio::test]
async fn typed_servers_on_this_computer_may_be_plain() {
    let mail = nothing_published().await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let typed = vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, "127.0.0.1"),
        plain(FieldKind::Security, "plain"),
        plain(FieldKind::Port, "31143"),
        plain(FieldKind::OutgoingServer, "127.0.0.1"),
        plain(FieldKind::OutgoingSecurity, "plain"),
        plain(FieldKind::OutgoingPort, "31587"),
    ];
    let steps = drive(&mut signin, manual_person(typed)).await;
    assert_eq!(
        shown(done(&steps)),
        vec![
            (
                Family::Imap,
                "imap://127.0.0.1:31143".into(),
                Tls::Plain,
                "ada@fake.test".into()
            ),
            (
                Family::Smtp,
                "smtp://127.0.0.1:31587".into(),
                Tls::Plain,
                "ada@fake.test".into()
            ),
        ]
    );
}

#[tokio::test]
async fn typed_plain_off_this_computer_is_not_accepted() {
    let mail = nothing_published().await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let typed = vec![
        plain(FieldKind::Protocol, "imap"),
        plain(FieldKind::Server, "mail.fake.test"),
        plain(FieldKind::Security, "plain"),
        plain(FieldKind::OutgoingServer, "mail.fake.test"),
        plain(FieldKind::OutgoingSecurity, "tls"),
    ];
    let steps = drive(&mut signin, manual_person(typed)).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreadable))
    );
}

/// A JMAP server the form names: a session resource that wants Basic with the login name.
#[derive(Debug, Clone)]
struct FakeJmap {
    login: &'static str,
    password: &'static str,
    api_user: &'static str,
    status: u16,
    /// An API token the session wants as a bearer, instead of Basic.
    token: Option<&'static str>,
}

impl porter_http::Http for FakeJmap {
    async fn send(
        &self,
        request: porter_http::HttpRequest,
    ) -> Result<porter_http::HttpResponse, porter_http::HttpError> {
        use base64::Engine;
        let want = match self.token {
            Some(token) => format!("Bearer {token}"),
            None => format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", self.login, self.password))
            ),
        };
        let ok = request
            .headers
            .iter()
            .any(|h| h.name.as_str().eq_ignore_ascii_case("authorization") && h.value.0 == want);
        let status = match (self.status, ok) {
            (200, true) => 200,
            (200, false) => 401,
            (other, _) => other,
        };
        let body = format!(
            r#"{{"capabilities":{{"urn:ietf:params:jmap:core":{{}},"urn:ietf:params:jmap:mail":{{}},"urn:ietf:params:jmap:submission":{{}}}},"apiUrl":"https://jmap.fake.test/api","username":"{}"}}"#,
            self.api_user
        );
        Ok(porter_http::HttpResponse {
            status: porter_http::Status(status),
            headers: vec![],
            body: body.into_bytes(),
        })
    }
}

fn jmap_provider(fake: FakeJmap) -> GenericProvider {
    GenericProvider::new(imap_spec(), SharedHttp::new(fake), FakeDns::new())
}

fn jmap_answers(login: Option<&str>) -> Vec<FieldAnswer> {
    let mut typed = vec![
        plain(FieldKind::Protocol, "jmap"),
        plain(FieldKind::SessionUrl, "https://jmap.fake.test/session"),
    ];
    typed.extend(login.map(|l| plain(FieldKind::Username, l)));
    typed
}

#[tokio::test]
async fn typed_jmap_reads_the_session_with_the_password_and_the_login_name() {
    let provider = jmap_provider(FakeJmap {
        login: "ada.login",
        password: "s3cret",
        api_user: "someone-else",
        status: 200,
        token: None,
    });
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, manual_person(jmap_answers(Some("ada.login")))).await;
    let signed = done(&steps);
    assert_eq!(
        shown(signed),
        vec![(
            Family::Jmap,
            "https://jmap.fake.test/api".into(),
            Tls::Implicit,
            "ada.login".into()
        )],
        "the login that authenticated, not the one the session reports"
    );
    assert!(
        signed
            .claims
            .iter()
            .any(|c| c.offer.kind() == CapabilityKind::Mail)
    );
}

#[tokio::test]
async fn typed_jmap_with_no_login_name_signs_in_as_the_address() {
    let provider = jmap_provider(FakeJmap {
        login: "ada@fake.test",
        password: "s3cret",
        api_user: "ada@fake.test",
        status: 200,
        token: None,
    });
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, manual_person(jmap_answers(None))).await;
    assert_eq!(done(&steps).endpoints[0].login.0, "ada@fake.test");
}

#[tokio::test]
async fn typed_jmap_refused_unreachable_and_unreadable_are_said() {
    for (status, fault) in [
        (401, SignInFault::Refused),
        (503, SignInFault::Unreachable),
        (404, SignInFault::Unreadable),
    ] {
        let provider = jmap_provider(FakeJmap {
            login: "ada@fake.test",
            password: "s3cret",
            api_user: "x",
            status,
            token: None,
        });
        let mut signin = provider.sign_in(add()).expect("sign-in");
        let steps = drive(&mut signin, manual_person(jmap_answers(None))).await;
        assert_eq!(steps.last(), Some(&SignInStep::Failed(fault)), "{status}");
    }
    // A wrong password is a refusal too.
    let provider = jmap_provider(FakeJmap {
        login: "ada@fake.test",
        password: "other",
        api_user: "x",
        status: 200,
        token: None,
    });
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, manual_person(jmap_answers(None))).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused))
    );
}

#[tokio::test]
async fn typed_jmap_with_an_api_token_presents_a_bearer_and_stores_the_token() {
    let provider = jmap_provider(FakeJmap {
        login: "ada@fake.test",
        password: "s3cret",
        api_user: "ada@fake.test",
        status: 200,
        token: Some("api-token-1"),
    });
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let mut typed = jmap_answers(None);
    typed.push(secret(FieldKind::Token, "api-token-1"));
    let steps = drive(&mut signin, manual_person(typed)).await;
    let signed = done(&steps);
    assert_eq!(
        signed.credentials,
        vec![(
            SecretPurpose::Password,
            Credential::Bearer(SecretText::new("api-token-1"))
        )],
        "the token is the account's secret, not the password typed first"
    );
    assert_eq!(signed.endpoints[0].family, Family::Jmap);

    // The password does not open a session that wants the token.
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, manual_person(jmap_answers(None))).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused))
    );
    // And a wrong token is refused.
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let mut typed = jmap_answers(None);
    typed.push(secret(FieldKind::Token, "wrong"));
    let steps = drive(&mut signin, manual_person(typed)).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused))
    );
}

#[tokio::test]
async fn typed_pop3_builds_pop3_and_smtp_endpoints_with_the_login_name() {
    let mail = mail_as("ada.login", FakeDns::new()).await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let typed = vec![
        plain(FieldKind::Protocol, "pop3"),
        plain(FieldKind::Server, "pop.fake.test"),
        plain(FieldKind::Security, "tls"),
        plain(FieldKind::OutgoingServer, "smtp.fake.test"),
        plain(FieldKind::OutgoingSecurity, "starttls"),
        plain(FieldKind::Username, "ada.login"),
    ];
    let steps = drive(&mut signin, manual_person(typed)).await;
    let signed = done(&steps);
    assert_eq!(
        shown(signed),
        vec![
            (
                Family::Pop3,
                "pop3s://pop.fake.test:995".into(),
                Tls::Implicit,
                "ada.login".into()
            ),
            (
                Family::Smtp,
                "smtp://smtp.fake.test:587".into(),
                Tls::StartTls,
                "ada.login".into()
            ),
        ]
    );
    assert!(signed.endpoints.iter().all(|e| e.check().is_ok()));
    assert_eq!(
        signed.credentials,
        vec![(
            SecretPurpose::Password,
            Credential::Password(SecretText::new("s3cret"))
        )]
    );
}

#[tokio::test]
async fn typed_pop3_starttls_defaults_to_port_110_and_may_be_plain_on_this_computer() {
    for (security, host, port, scheme, tls) in [
        (
            "starttls",
            "pop.fake.test",
            "",
            "pop3://pop.fake.test:110",
            Tls::StartTls,
        ),
        (
            "plain",
            "127.0.0.1",
            "31110",
            "pop3://127.0.0.1:31110",
            Tls::Plain,
        ),
    ] {
        let mail = nothing_published().await;
        let mut signin = mail.provider.sign_in(add()).expect("sign-in");
        let typed = vec![
            plain(FieldKind::Protocol, "pop3"),
            plain(FieldKind::Server, host),
            plain(FieldKind::Security, security),
            plain(FieldKind::Port, port),
            plain(FieldKind::OutgoingServer, host),
            plain(FieldKind::OutgoingSecurity, security),
        ];
        let steps = drive(&mut signin, manual_person(typed)).await;
        let first = &done(&steps).endpoints[0];
        assert_eq!(
            (first.family, first.url.to_string(), first.tls),
            (Family::Pop3, scheme.into(), tls)
        );
    }
}

#[tokio::test]
async fn nothing_reachable_is_offline_and_a_bad_form_is_unreadable() {
    // No host is in the table, so every request fails to connect, and so does DNS.
    let provider = GenericProvider::new(
        imap_spec(),
        SharedHttp::new(Fakes::loopback()),
        FakeDns::new().unreachable(),
    );
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", "pw", "")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreachable)),
        "{steps:?}"
    );

    let mail = mail_with(FakeDns::new()).await;
    for (address, password) in [("not-an-address", "pw"), ("ada@fake.test", ""), ("", "pw")] {
        let mut signin = mail.provider.sign_in(add()).expect("sign-in");
        let steps = drive(&mut signin, mail_person(address, password, "")).await;
        assert_eq!(
            steps.last(),
            Some(&SignInStep::Failed(SignInFault::Unreadable)),
            "{address:?}"
        );
    }
    // A typed server that is not a name.
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        mail_person("ada@fake.test", "pw", "not a host"),
    )
    .await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreadable))
    );
}

#[tokio::test]
async fn signing_in_again_to_a_mail_account_asks_the_form_and_finishes_without_a_review() {
    let mail = mail_with(FakeDns::new()).await;
    mail.autoconfig.serve_autoconfig(&autoconfig_xml(
        "fake.test",
        ("imap.fake.test", 993),
        ("smtp.fake.test", 587),
    ));
    let start = SignInStart::new(SignInMode::Reauthenticate {
        account: AccountId::parse("generic-imap-ada").expect("id"),
        endpoints: vec![],
    });
    let mut signin = mail.provider.sign_in(start).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", PASSWORD, "")).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert!(matches!(steps[0], SignInStep::AskFields(_)));
    let signed = done(&steps);
    assert!(matches!(&signed.credentials[0].1, Credential::Password(p) if p.expose() == PASSWORD));
}

struct Dav {
    dav: Running<DavHandle>,
    wellknown: Running<AutoconfigHandle>,
    provider: GenericProvider,
}

async fn dav() -> Dav {
    dav_with(true).await
}

/// A DAV server whose `.well-known` names the calendar and, when `address_book`, the contacts.
async fn dav_with(address_book: bool) -> Dav {
    let dav = FakeDav::start("bob", "hunter2").await.expect("dav");
    let wellknown = FakeAutoconfig::start().await.expect("well-known");
    wellknown.serve_redirect("/.well-known/caldav", "/dav/calendar/");
    if address_book {
        wellknown.serve_redirect("/.well-known/carddav", "/dav/contacts/");
    }
    let places = vec![
        Place {
            host: "dav.fake.test",
            prefix: "/.well-known",
            port: port_of(wellknown.base_url()),
        },
        Place {
            host: "dav.fake.test",
            prefix: "/dav",
            port: port_of(dav.base_url()),
        },
    ];
    let provider = GenericProvider::new(
        dav_spec(),
        SharedHttp::new(Fakes::new(places)),
        FakeDns::new(),
    );
    Dav {
        dav,
        wellknown,
        provider,
    }
}

fn dav_person<'a>(
    server: &'a str,
    user: &'a str,
    password: &'a str,
) -> impl FnMut(&SignInStep) -> SignInInput + 'a {
    move |step| match step {
        SignInStep::AskFields(_) => SignInInput::Fields(vec![
            plain(FieldKind::Server, server),
            plain(FieldKind::Username, user),
            secret(FieldKind::Password, password),
        ]),
        SignInStep::Review { claims, .. } => {
            SignInInput::Confirm(all_on(claims.iter().map(|c| c.offer.kind())))
        }
        _ => SignInInput::Cancel,
    }
}

#[tokio::test]
async fn a_dav_account_is_found_from_the_servers_name_by_well_known() {
    let world = dav().await;
    let mut signin = world.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, dav_person("dav.fake.test", "bob", "hunter2")).await;

    let SignInStep::AskFields(form) = &steps[0] else {
        panic!("a form first: {steps:?}");
    };
    let asked: Vec<FieldKind> = form.iter().map(|f| f.kind).collect();
    assert_eq!(
        asked,
        [FieldKind::Server, FieldKind::Username, FieldKind::Password]
    );
    let signed = done(&steps);
    assert_eq!(signed.label.0, "bob@dav.fake.test");
    let endpoints: Vec<_> = signed
        .endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.as_str()))
        .collect();
    assert_eq!(
        endpoints,
        vec![
            (
                Family::CalDav,
                "https://dav.fake.test/dav/calendar/".to_owned(),
                Tls::Implicit,
                "bob"
            ),
            (
                Family::CardDav,
                "https://dav.fake.test/dav/contacts/".to_owned(),
                Tls::Implicit,
                "bob"
            ),
        ]
    );
    let kinds: Vec<_> = signed
        .claims
        .iter()
        .map(|c| (c.offer.kind(), matches!(c.offer, Offer::Present(_))))
        .collect();
    assert_eq!(
        kinds,
        [
            (CapabilityKind::Calendar, true),
            (CapabilityKind::Contacts, true)
        ]
    );
    // The password went to the DAV server only, and with the PROPFIND that checked it.
    let hits = world.dav.hits();
    assert!(hits.iter().all(|h| h.authorization.is_some()), "{hits:?}");
    assert_eq!(
        world
            .wellknown
            .hits()
            .iter()
            .filter(|h| h.authorization.is_some())
            .count(),
        0,
        "the well-known lookups carry no credential"
    );
}

#[tokio::test]
async fn an_address_with_a_path_is_the_root_of_both_services() {
    let world = dav().await;
    let root = format!("{}/dav/calendar/", world.dav.base_url());
    let mut signin = world.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, dav_person(&root, "bob", "hunter2")).await;
    let signed = done(&steps);
    // The same root serves both families, which both answer a PROPFIND: two endpoints, one URL.
    assert_eq!(signed.endpoints.len(), 2);
    assert!(signed.endpoints.iter().all(|e| e.url.to_string() == root));
    assert_eq!(signed.endpoints[0].tls, Tls::Plain);
    assert_eq!(signed.label.0, "bob@127.0.0.1");
}

#[tokio::test]
async fn a_service_the_server_does_not_publish_is_marked_absent() {
    let world = dav_with(false).await;
    let mut signin = world.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, dav_person("dav.fake.test", "bob", "hunter2")).await;
    let signed = done(&steps);
    let families: Vec<Family> = signed.endpoints.iter().map(|e| e.family).collect();
    assert_eq!(families, [Family::CalDav]);
    let contacts = signed
        .claims
        .iter()
        .find(|c| c.offer.kind() == CapabilityKind::Contacts)
        .expect("a contacts claim");
    assert!(
        matches!(
            contacts.offer,
            Offer::Absent {
                reason: porter_core::AbsentReason::NotOnServer,
                ..
            }
        ),
        "{contacts:?}"
    );
}

#[tokio::test]
async fn a_refused_dav_password_ends_the_sign_in_refused_and_a_server_with_no_dav_fails() {
    let world = dav().await;
    let mut signin = world.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, dav_person("dav.fake.test", "bob", "wrong")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused))
    );

    // A server that cannot be reached, and one that answers `.well-known` with nothing.
    let mut signin = world.provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        dav_person("nowhere.fake.test", "bob", "hunter2"),
    )
    .await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreachable))
    );
    let bare = FakeAutoconfig::start().await.expect("bare");
    let mut signin = world.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, dav_person(bare.base_url(), "bob", "hunter2")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreadable))
    );

    for (server, user, password) in [
        ("", "bob", "pw"),
        ("dav.fake.test", "", "pw"),
        ("dav.fake.test", "bob", ""),
    ] {
        let mut signin = world.provider.sign_in(add()).expect("sign-in");
        let steps = drive(&mut signin, dav_person(server, user, password)).await;
        assert_eq!(
            steps.last(),
            Some(&SignInStep::Failed(SignInFault::Unreadable))
        );
    }
}

#[tokio::test]
async fn a_provider_file_that_is_not_a_generic_kind_does_not_sign_in() {
    let provider = GenericProvider::new(
        porter_fake_servers::shipped::nextcloud(),
        SharedHttp::new(Fakes::loopback()),
        FakeDns::new(),
    );
    assert_eq!(
        provider.sign_in(add()).err(),
        Some(ProviderError::Unreadable)
    );
}

#[tokio::test]
async fn a_session_never_hands_out_the_password_and_revoke_has_nothing_to_do() {
    let mail = mail_with(FakeDns::new()).await;
    let account = AccountId::parse("generic-imap-ada").expect("id");
    let presented = Presented::Credential(Credential::Password(SecretText::new("pw")));
    let session = mail
        .provider
        .open(&account, presented.clone())
        .await
        .expect("session");
    assert_eq!(session.account(), &account);
    assert_eq!(session.renewed(), None);
    assert_eq!(
        session
            .access_token(&porter_core::Audience("imap".into()))
            .await
            .err(),
        Some(ProviderError::Forbidden)
    );
    assert_eq!(
        mail.provider
            .open(&account, Presented::Anonymous)
            .await
            .err(),
        Some(ProviderError::Unauthorized)
    );
    assert_eq!(
        mail.provider
            .revoke(
                &held_by(&account, porter_core::AuthKind::Password, "generic-imap"),
                &presented
            )
            .await,
        Ok(porter_provider::RevokeOutcome::Unsupported)
    );
    let claims = mail
        .provider
        .discover(
            &held_by(&account, porter_core::AuthKind::Password, "generic-imap"),
            &presented,
        )
        .await
        .expect("claims");
    assert_eq!(claims.len(), 1);
}

fn held_by(id: &AccountId, auth: porter_core::AuthKind, provider: &str) -> porter_core::Account {
    porter_core::Account {
        id: id.clone(),
        provider: porter_core::ProviderId::parse(provider).expect("provider"),
        label: porter_core::AccountLabel("held".into()),
        state: porter_core::AccountState::Ok,
        auth,
        capabilities: vec![],
        restriction: porter_core::Restriction::none(),
        endpoints: vec![],
    }
}

// ---- a brand file with fixed endpoints ----

fn brand(id: &str) -> ProviderSpec {
    let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../providers");
    let text =
        std::fs::read_to_string(shipped.join(format!("{id}.toml"))).expect("the shipped file");
    spec(&text)
}

fn fixed_provider(
    spec: ProviderSpec,
    connect: impl porter_proxy::Connect + 'static,
) -> GenericProvider {
    GenericProvider::new(
        spec,
        SharedHttp::new(Fakes::loopback()),
        FakeDns::new().unreachable(),
    )
    .with_connect(connect)
}

/// Answers the first form with `address` and `password` (in the password field the form shows:
/// an app password for the providers that take only one), and the review with everything on.
fn fixed_person<'a>(
    address: &'a str,
    password: &'a str,
) -> impl FnMut(&SignInStep) -> SignInInput + 'a {
    move |step| match step {
        SignInStep::AskFields(fields) => SignInInput::Fields(vec![
            plain(FieldKind::Address, address),
            secret(
                fields
                    .iter()
                    .map(|f| f.kind)
                    .find(|k| matches!(k, FieldKind::Password | FieldKind::AppPassword))
                    .unwrap_or(FieldKind::Password),
                password,
            ),
        ]),
        SignInStep::Review { claims, .. } => {
            SignInInput::Confirm(all_on(claims.iter().map(|c| c.offer.kind())))
        }
        _ => SignInInput::Cancel,
    }
}

async fn line(stream: &mut tokio::io::BufReader<tokio::net::TcpStream>) -> String {
    use tokio::io::AsyncBufReadExt;
    let mut text = String::new();
    stream.read_line(&mut text).await.expect("a line");
    text
}

async fn say(stream: &mut tokio::io::BufReader<tokio::net::TcpStream>, text: &str) {
    use tokio::io::AsyncWriteExt;
    stream.write_all(text.as_bytes()).await.expect("write");
}

/// What a client does with a signed account's endpoint: connect to the host and port it names,
/// log in with the login name and the password the sign-in produced.
async fn dial(url: &str) -> tokio::io::BufReader<tokio::net::TcpStream> {
    let authority = url.split_once("://").expect("scheme").1;
    let stream = tokio::net::TcpStream::connect(authority.trim_end_matches('/'))
        .await
        .expect("the fake");
    tokio::io::BufReader::new(stream)
}

#[tokio::test]
async fn a_fixed_file_signs_in_to_the_servers_it_names_and_the_password_logs_in() {
    let accounts = Accounts::password("ada@fastmail.com", "app-pw");
    let imap = FakeImap::bind(&Bind::Loopback, Tls::Plain, accounts.clone(), mailbox(1))
        .await
        .expect("imap");
    let smtp = FakeSmtp::bind(&Bind::Loopback, Tls::Plain, accounts)
        .await
        .expect("smtp");
    let file = smtp.rewrite(&imap.rewrite(&brand("fastmail")));
    let (imap_handle, smtp_handle) = (imap.handle(), smtp.handle());
    let (_imap, _smtp) = (
        Running::spawn(imap, imap_handle.clone()),
        Running::spawn(smtp, smtp_handle.clone()),
    );

    let provider = fixed_provider(file, porter_proxy::RustlsConnect::trusting([]));
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, fixed_person("ada@fastmail.com", "app-pw")).await;
    let asked: Vec<FieldKind> = match &steps[0] {
        SignInStep::AskFields(form) => form.iter().map(|f| f.kind).collect(),
        other => panic!("a form first: {other:?}"),
    };
    // Fastmail takes only an app password from a mail app: the form says so (ship-5).
    assert_eq!(asked, [FieldKind::Address, FieldKind::AppPassword]);
    let signed = done(&steps);
    assert_eq!(signed.label.0, "ada@fastmail.com");
    let [(SecretPurpose::Password, Credential::Password(password))] = signed.credentials.as_slice()
    else {
        panic!("one password: {:?}", signed.credentials);
    };
    assert!(
        signed
            .endpoints
            .iter()
            .all(|e| e.login.0 == "ada@fastmail.com")
    );

    let of = |family| {
        signed
            .endpoints
            .iter()
            .find(|e| e.family == family)
            .unwrap_or_else(|| panic!("{family:?}"))
    };
    let (imap_end, smtp_end) = (of(Family::Imap), of(Family::Smtp));
    assert_eq!((imap_end.tls, smtp_end.tls), (Tls::Plain, Tls::Plain));

    let mut stream = dial(imap_end.url.as_str()).await;
    assert!(line(&mut stream).await.starts_with("* OK"));
    say(
        &mut stream,
        &format!(
            "a1 LOGIN \"{}\" \"{}\"\r\n",
            imap_end.login.0,
            password.expose()
        ),
    )
    .await;
    assert!(line(&mut stream).await.contains("a1 OK"));
    let mut stream = dial(smtp_end.url.as_str()).await;
    assert!(line(&mut stream).await.starts_with("220"));
    say(&mut stream, "EHLO test\r\n").await;
    while !line(&mut stream).await.starts_with("250 ") {}
    let plain =
        porter_fake_servers::mail::b64(&format!("\0{}\0{}", smtp_end.login.0, password.expose()));
    say(&mut stream, &format!("AUTH PLAIN {plain}\r\n")).await;
    assert!(line(&mut stream).await.starts_with("235"));

    // The IMAP server saw the sign-in's own login before the review, then the one above.
    let attempts = imap_handle.attempts();
    assert!(
        matches!(attempts.as_slice(), [first, second]
            if first.accepted && second.accepted && first.user == "ada@fastmail.com" && second.user == first.user),
        "imap: {attempts:?}"
    );
    let attempts = smtp_handle.attempts();
    assert!(
        matches!(attempts.as_slice(), [a] if a.accepted && a.user == "ada@fastmail.com"),
        "smtp: {attempts:?}"
    );
}

/// ship-5: the providers whose service takes only an app password from a mail app ask for an
/// "App password"; one that takes the account's password asks for a password.
#[tokio::test]
async fn the_providers_that_take_only_an_app_password_ask_for_one() {
    for (file, want) in [
        ("icloud", FieldKind::AppPassword),
        ("fastmail", FieldKind::AppPassword),
        ("yahoo", FieldKind::AppPassword),
        ("gmx", FieldKind::Password),
    ] {
        let provider = fixed_provider(brand(file), porter_proxy::RustlsConnect::trusting([]));
        let mut signin = provider.sign_in(add()).expect("sign-in");
        let SignInStep::AskFields(form) =
            porter_provider::SignIn::next(&mut signin, SignInInput::Start).await
        else {
            panic!("{file}: a form first");
        };
        let kinds: Vec<FieldKind> = form.iter().map(|f| f.kind).collect();
        assert_eq!(kinds, [FieldKind::Address, want], "{file}");
    }
}

#[tokio::test]
async fn the_review_of_a_fixed_file_shows_the_files_endpoints_and_claims() {
    let servers = MailWorld::start("ada@icloud.com", "app-pw").await;
    let provider = fixed_provider(brand("icloud"), servers.wire());
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, fixed_person("ada@icloud.com", "app-pw")).await;
    let SignInStep::Review {
        claims,
        endpoints,
        label,
        ..
    } = &steps[1]
    else {
        panic!("a review after the form: {steps:?}");
    };
    assert_eq!(label.0, "ada@icloud.com");
    let shown: Vec<_> = endpoints
        .iter()
        .map(|e| (e.family, e.url.to_string(), e.tls, e.login.0.as_str()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (
                Family::Imap,
                "imaps://imap.mail.me.com:993".to_owned(),
                Tls::Implicit,
                "ada@icloud.com"
            ),
            (
                Family::Smtp,
                "smtp://smtp.mail.me.com:587".to_owned(),
                Tls::StartTls,
                "ada@icloud.com"
            ),
            (
                Family::CalDav,
                "https://caldav.icloud.com".to_owned(),
                Tls::Implicit,
                "ada@icloud.com"
            ),
            (
                Family::CardDav,
                "https://contacts.icloud.com".to_owned(),
                Tls::Implicit,
                "ada@icloud.com"
            ),
        ]
    );
    let kinds: Vec<_> = claims.iter().map(|c| c.offer.kind()).collect();
    assert_eq!(
        kinds,
        [
            CapabilityKind::Mail,
            CapabilityKind::Calendar,
            CapabilityKind::Contacts
        ],
        "the file's claims, the mail capability once"
    );
    assert!(claims.iter().all(|c| matches!(c.offer, Offer::Present(_))));
}

#[tokio::test]
async fn a_fixed_file_refuses_a_bad_form_and_signs_in_again_without_a_review() {
    let servers = MailWorld::start("ada@gmx.de", "new").await;
    let provider = fixed_provider(brand("gmx"), servers.wire());
    for (address, password) in [("not-an-address", "pw"), ("ada@gmx.de", ""), ("", "pw")] {
        let mut signin = provider.sign_in(add()).expect("sign-in");
        let steps = drive(&mut signin, fixed_person(address, password)).await;
        assert_eq!(
            steps.last(),
            Some(&SignInStep::Failed(SignInFault::Unreadable)),
            "{address:?}"
        );
    }
    let start = SignInStart::new(SignInMode::Reauthenticate {
        account: AccountId::parse("gmx-ada").expect("id"),
        endpoints: vec![],
    });
    let mut signin = provider.sign_in(start).expect("sign-in");
    let steps = drive(&mut signin, fixed_person("ada@gmx.de", "new")).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert!(
        matches!(&done(&steps).credentials[0].1, Credential::Password(p) if p.expose() == "new")
    );
}

#[tokio::test]
async fn a_fastmail_address_through_generic_imap_still_reports_the_fastmail_provider() {
    use porter_discover::{Outcome, discover_mail};
    let providers =
        porter_provider::ProviderSet::layered(vec![brand("fastmail"), brand("gmx")], vec![]);
    let http = SharedHttp::new(Fakes::loopback());
    let dns = FakeDns::new();
    for (address, id) in [("ada@fastmail.com", "fastmail"), ("ada@gmx.de", "gmx")] {
        match discover_mail(&http, &dns, &providers, address).await {
            Ok(Outcome::Provider(lead)) => assert_eq!(lead.provider.as_str(), id, "{address}"),
            other => panic!("{address}: {other:?}"),
        }
    }
}

// ---- the password is tried at the mail server before the review ----

/// A mail account published by autoconfig, with the fakes behind it.
async fn published() -> Mail {
    let mail = mail_with(FakeDns::new()).await;
    mail.autoconfig.serve_autoconfig(&autoconfig_xml(
        "fake.test",
        ("imap.fake.test", 993),
        ("smtp.fake.test", 587),
    ));
    mail
}

fn has_review(steps: &[SignInStep]) -> bool {
    steps.iter().any(|s| matches!(s, SignInStep::Review { .. }))
}

#[tokio::test]
async fn a_wrong_mail_password_is_refused_before_the_review() {
    let mail = published().await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", "wrong", "")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused)),
        "{steps:?}"
    );
    assert!(!has_review(&steps), "{steps:?}");
}

#[tokio::test]
async fn a_mail_server_that_cannot_be_reached_is_unreachable_before_the_review() {
    let mail = published().await;
    let provider = mail.provider.clone().with_connect(Wire::nowhere());
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", PASSWORD, "")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreachable)),
        "{steps:?}"
    );
    assert!(!has_review(&steps), "{steps:?}");
}

#[tokio::test]
async fn the_right_mail_password_goes_on_to_the_review() {
    let mail = published().await;
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", PASSWORD, "")).await;
    assert!(has_review(&steps), "{steps:?}");
    done(&steps);
}

#[tokio::test]
async fn a_wrong_pop3_password_is_refused_and_a_right_one_goes_on() {
    let mail = nothing_published().await;
    let typed = |_: ()| {
        vec![
            plain(FieldKind::Protocol, "pop3"),
            plain(FieldKind::Server, "pop.fake.test"),
            plain(FieldKind::Security, "starttls"),
            plain(FieldKind::OutgoingServer, "smtp.fake.test"),
            plain(FieldKind::OutgoingSecurity, "starttls"),
        ]
    };
    let person = |password: &'static str| {
        let mut asked = manual_person(typed(()));
        move |step: &SignInStep| match step {
            SignInStep::AskFields(fields)
                if fields.iter().any(|f| f.kind == FieldKind::Address) =>
            {
                SignInInput::Fields(vec![
                    plain(FieldKind::Address, "ada@fake.test"),
                    secret(FieldKind::Password, password),
                ])
            }
            other => asked(other),
        }
    };
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, person("wrong")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused)),
        "{steps:?}"
    );
    assert!(!has_review(&steps), "{steps:?}");
    let mut signin = mail.provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, person(PASSWORD)).await;
    assert!(has_review(&steps), "{steps:?}");
}

#[tokio::test]
async fn signing_in_again_with_a_wrong_mail_password_is_refused() {
    let mail = published().await;
    let start = SignInStart::new(SignInMode::Reauthenticate {
        account: AccountId::parse("generic-imap-ada").expect("id"),
        endpoints: vec![],
    });
    let mut signin = mail.provider.sign_in(start).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", "wrong", "")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused)),
        "{steps:?}"
    );
}

#[tokio::test]
async fn a_fixed_file_checks_the_password_too() {
    let servers = MailWorld::start("ada@gmx.de", "app-pw").await;
    let provider = fixed_provider(brand("gmx"), servers.wire());
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, fixed_person("ada@gmx.de", "wrong")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused)),
        "{steps:?}"
    );
    assert!(!has_review(&steps), "{steps:?}");

    let provider = fixed_provider(brand("gmx"), Wire::nowhere());
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(&mut signin, fixed_person("ada@gmx.de", "app-pw")).await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Unreachable)),
        "{steps:?}"
    );
}

// ---- servers only unsigned DNS names, outside the address's domain (sec-2) ----

/// A connector that counts the connections the sign-in opens, then reaches the fakes.
#[derive(Debug, Clone)]
struct Counted {
    wire: Wire,
    dials: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Counted {
    fn dials(&self) -> usize {
        self.dials.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl porter_proxy::Connect for Counted {
    type Stream = porter_proxy::NetStream;

    async fn dial(
        &self,
        origin: &porter_core::Origin,
        tls: Tls,
    ) -> Result<porter_proxy::NetStream, porter_proxy::ConnectFault> {
        self.dials.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.wire.dial(origin, tls).await
    }

    async fn upgrade(
        &self,
        stream: porter_proxy::NetStream,
        host: &str,
    ) -> Result<porter_proxy::NetStream, porter_proxy::ConnectFault> {
        self.wire.upgrade(stream, host).await
    }
}

fn srv(name: &str, port: u16, target: &str) -> (String, Vec<SrvRecord>) {
    let record = SrvRecord {
        priority: 0,
        weight: 1,
        port,
        target: porter_provider::DomainName::parse(target).expect("name"),
    };
    (name.to_owned(), vec![record])
}

/// DNS that answers `fake.test`'s SRV names with servers at `host`, and nothing else.
fn srv_at(imap: &str, smtp: &str) -> FakeDns {
    let (imaps, imaps_at) = srv("_imaps._tcp.fake.test", 993, imap);
    let (submission, submission_at) = srv("_submission._tcp.fake.test", 587, smtp);
    FakeDns::new()
        .with_srv(&imaps, imaps_at)
        .with_srv(&submission, submission_at)
}

/// A provider over `dns` whose connections are counted.
async fn counted(dns: FakeDns) -> (Mail, GenericProvider, Counted) {
    let mail = mail_with(dns).await;
    let counted = Counted {
        wire: mail._servers.wire(),
        dials: Default::default(),
    };
    let provider = mail.provider.clone().with_connect(counted.clone());
    (mail, provider, counted)
}

/// Answers as [`mail_person`] does, noting how many connections were open when the review came.
fn noting_person<'a>(
    password: &'a str,
    counted: &'a Counted,
    at_review: &'a std::cell::Cell<Option<usize>>,
    confirm: bool,
) -> impl FnMut(&SignInStep) -> SignInInput + 'a {
    let mut person = mail_person("ada@fake.test", password, "mail.fake.test");
    move |step| match step {
        SignInStep::Review { .. } => {
            at_review.set(Some(counted.dials()));
            match confirm {
                true => person(step),
                false => SignInInput::Cancel,
            }
        }
        other => person(other),
    }
}

#[tokio::test]
async fn srv_servers_outside_the_domain_are_shown_before_the_password_is_sent_anywhere() {
    let (_mail, provider, counted) =
        counted(srv_at("imap.elsewhere.test", "smtp.elsewhere.test")).await;
    let at_review = std::cell::Cell::new(None);

    // The person sees the servers and closes the sheet: nothing was ever dialled.
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        noting_person(PASSWORD, &counted, &at_review, false),
    )
    .await;
    let SignInStep::Review { endpoints, .. } = &steps[1] else {
        panic!("the review second: {steps:?}");
    };
    assert_eq!(
        endpoints[0].url.to_string(),
        "imaps://imap.elsewhere.test:993"
    );
    assert_eq!(at_review.get(), Some(0), "{steps:?}");
    assert_eq!(counted.dials(), 0);

    // Confirmed with a wrong password: the password is tried then, and refused.
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        noting_person("wrong", &counted, &at_review, true),
    )
    .await;
    assert_eq!(at_review.get(), Some(0), "{steps:?}");
    assert!(counted.dials() > 0);
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused)),
        "{steps:?}"
    );

    // Confirmed with the right one: done.
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        noting_person(PASSWORD, &counted, &at_review, true),
    )
    .await;
    assert!(has_review(&steps), "{steps:?}");
    assert_eq!(
        done(&steps).endpoints[1].url.to_string(),
        "smtp://smtp.elsewhere.test:587"
    );
}

#[tokio::test]
async fn srv_servers_within_the_domain_still_have_the_password_tried_before_the_review() {
    let (_mail, provider, counted) = counted(srv_at("imap.fake.test", "smtp.fake.test")).await;
    let at_review = std::cell::Cell::new(None);
    let mut signin = provider.sign_in(add()).expect("sign-in");
    let steps = drive(
        &mut signin,
        noting_person("wrong", &counted, &at_review, true),
    )
    .await;
    assert_eq!(
        steps.last(),
        Some(&SignInStep::Failed(SignInFault::Refused)),
        "{steps:?}"
    );
    assert!(!has_review(&steps), "{steps:?}");
}

#[tokio::test]
async fn signing_in_again_uses_srv_servers_outside_the_domain_only_when_the_account_has_them() {
    let (_mail, provider, counted) =
        counted(srv_at("imap.elsewhere.test", "smtp.elsewhere.test")).await;
    let again = |endpoints| {
        SignInStart::new(SignInMode::Reauthenticate {
            account: AccountId::parse("generic-imap-ada").expect("id"),
            endpoints,
        })
    };
    let at_review = std::cell::Cell::new(None);

    // Servers the account never had: the person is asked for the server, nothing is dialled
    // before the answer.
    use porter_provider::SignIn as _;
    let mut signin = provider.sign_in(again(Vec::new())).expect("sign-in");
    let first = signin.next(SignInInput::Start).await;
    assert!(matches!(first, SignInStep::AskFields(_)), "{first:?}");
    let asked = signin
        .next(SignInInput::Fields(vec![
            plain(FieldKind::Address, "ada@fake.test"),
            secret(FieldKind::Password, PASSWORD),
        ]))
        .await;
    let SignInStep::AskFields(fields) = &asked else {
        panic!("the question for a server: {asked:?}");
    };
    assert!(
        fields.iter().any(|f| f.kind == FieldKind::Server),
        "{fields:?}"
    );
    assert_eq!(counted.dials(), 0);

    // The servers the account has: tried, and done.
    let held = {
        let mut signin = provider.sign_in(add()).expect("sign-in");
        let steps = drive(
            &mut signin,
            noting_person(PASSWORD, &counted, &at_review, true),
        )
        .await;
        done(&steps).endpoints.clone()
    };
    let mut signin = provider.sign_in(again(held)).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", PASSWORD, "")).await;
    assert!(!has_review(&steps), "{steps:?}");
    done(&steps);
}
