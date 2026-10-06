//! The Generic family against the fake autoconfig, DNS and DAV servers: a mail account found from
//! its address (and from a typed server when nothing is published), a DAV account found from its
//! server's name or its address, a refused password, and the session.
#![cfg(feature = "generic")]

mod common;

use common::{Fakes, Place, all_on, drive, plain, secret};
use porter_core::capability::CapabilityKind;
use porter_core::sheet::{FieldKind, SignInFault, SignInInput};
use porter_core::{AccountId, Credential, Family, Offer, SecretPurpose, SecretText, Tls};
use porter_discover::{MxRecord, SrvRecord};
use porter_fake_servers::{
    AutoconfigHandle, DavHandle, FakeAutoconfig, FakeDav, FakeDns, Running, autoconfig_xml,
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
    spec(include_str!("../../../providers/generic-imap.toml"))
}

fn dav_spec() -> ProviderSpec {
    spec(include_str!("../../../providers/generic-dav.toml"))
}

fn add() -> SignInStart {
    SignInStart {
        mode: SignInMode::Add,
    }
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
}

async fn mail_with(dns: FakeDns) -> Mail {
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
        provider: GenericProvider::new(imap_spec(), SharedHttp::new(Fakes::new(places)), dns),
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
        SignInStep::AskFields(_) => SignInInput::Fields(vec![plain(FieldKind::Server, server)]),
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
    let steps = drive(&mut signin, mail_person("ada@fake.test", "pw", "")).await;
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
        mail_person("ada@fake.test", "pw", "mail.fake.test"),
    )
    .await;

    let questions: Vec<Vec<FieldKind>> = steps
        .iter()
        .filter_map(|s| match s {
            SignInStep::AskFields(f) => Some(f.iter().map(|f| f.kind).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(
        questions,
        vec![
            vec![FieldKind::Address, FieldKind::Password],
            vec![FieldKind::Server]
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
    let start = SignInStart {
        mode: SignInMode::Reauthenticate {
            account: AccountId::parse("generic-imap-ada").expect("id"),
            endpoints: vec![],
        },
    };
    let mut signin = mail.provider.sign_in(start).expect("sign-in");
    let steps = drive(&mut signin, mail_person("ada@fake.test", "new", "")).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert!(matches!(steps[0], SignInStep::AskFields(_)));
    let signed = done(&steps);
    assert!(matches!(&signed.credentials[0].1, Credential::Password(p) if p.expose() == "new"));
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
