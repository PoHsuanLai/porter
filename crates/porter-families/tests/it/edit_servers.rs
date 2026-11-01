//! Editing a generic mail account's servers (`SignInMode::EditServers`) against the fake IMAP
//! and SMTP servers: the incoming server is always logged in to, the outgoing one only when it,
//! the login name or the password changed, an empty password tries the password the host holds,
//! a refusal says what was wrong and the sign-in starts again, and no bearer token is ever sent
//! to a typed host.
#![cfg(feature = "generic")]

use crate::common::{Fakes, plain, secret};
use porter_core::{AccountId, AccountLabel, AuthKind, Family, SecretText, Tls};
use porter_fake_servers::net::{Bind, port_of};
use porter_fake_servers::{
    Accounts, FakeDns, FakeImap, FakeSmtp, MailHandle, Running, Secret, mailbox,
};
use porter_family_generic::{DavLookup, GenericProvider, MailLookup};
use porter_http::SharedHttp;
use porter_provider::{
    Provider, SignIn, SignInMode, SignInStart, SignInStep, Signed, parse_provider,
};
use porter_sheet::{
    FieldAnswer, FieldKind, Hop, MailServers, Manual, Security, SignInFault, SignInInput, edit_form,
};

const ADDRESS: &str = "ada@example.test";
const STORED: &str = "stored-pw-example";

struct World {
    imap: Running<MailHandle>,
    smtp: Running<MailHandle>,
    provider: GenericProvider,
}

async fn world() -> World {
    let accounts = Accounts::password(ADDRESS, STORED);
    let imap = FakeImap::start(&Bind::Port(0), Tls::Plain, accounts.clone(), mailbox(1))
        .await
        .expect("imap");
    let smtp = FakeSmtp::start(&Bind::Port(0), Tls::Plain, accounts)
        .await
        .expect("smtp");
    let spec = parse_provider(include_str!("../../../../providers/generic-imap.toml"))
        .expect("the shipped file");
    World {
        imap,
        smtp,
        provider: GenericProvider::new(
            spec,
            SharedHttp::new(Fakes::loopback()),
            FakeDns::new().unreachable(),
        )
        .with_connect(porter_proxy::RustlsConnect::trusting([])),
    }
}

fn on_this_computer(port: u16) -> Hop {
    Hop {
        host: "127.0.0.1".to_owned(),
        port,
        security: Security::Plain,
    }
}

/// The account's servers as stored: the fake IMAP server, and SMTP at `smtp_port`.
fn current(world: &World, smtp_port: u16) -> Manual {
    Manual::Imap(MailServers {
        incoming: on_this_computer(port_of(world.imap.address())),
        outgoing: on_this_computer(smtp_port),
        login: None,
    })
}

fn smtp_port(world: &World) -> u16 {
    port_of(world.smtp.address())
}

fn mode(current: Manual, keep: Option<&str>) -> SignInMode {
    SignInMode::EditServers {
        account: AccountId::parse("ada").expect("id"),
        auth: AuthKind::Password,
        label: AccountLabel(ADDRESS.to_owned()),
        current,
        name: None,
        keep: keep.map(SecretText::new),
    }
}

/// The edit form of `current` sent back as prefilled, with `changes` over it.
fn sent(current: &Manual, changes: Vec<FieldAnswer>) -> SignInInput {
    let mut answers: Vec<FieldAnswer> = edit_form(AuthKind::Password, current, None)
        .iter()
        .filter(|f| changes.iter().all(|c| c.kind != f.kind))
        .map(|f| plain(f.kind, f.prefill.as_deref().unwrap_or("")))
        .collect();
    answers.extend(changes);
    SignInInput::Fields(answers)
}

/// Starts a sign-in for `mode`, takes its form, and sends `input`.
async fn edit(world: &World, mode: SignInMode, input: SignInInput) -> SignInStep {
    let mut signin = world
        .provider
        .sign_in(SignInStart::new(mode))
        .expect("sign-in");
    let SignInStep::AskFields(_) = signin.next(SignInInput::Start).await else {
        panic!("the edit form first");
    };
    signin.next(input).await
}

fn done(step: SignInStep) -> Signed {
    match step {
        SignInStep::Done(signed) => signed,
        other => panic!("expected Done, got {other:?}"),
    }
}

fn is_password(secret: &Secret, want: &str) -> bool {
    *secret == Secret::Password(want.to_owned())
}

#[tokio::test]
async fn the_edit_form_is_asked_first_prefilled_from_the_account() {
    let world = world().await;
    let current = current(&world, smtp_port(&world));
    let mut signin = world
        .provider
        .sign_in(SignInStart::new(mode(current.clone(), None)))
        .expect("sign-in");
    let first = signin.next(SignInInput::Start).await;
    assert_eq!(
        first,
        SignInStep::AskFields(edit_form(AuthKind::Password, &current, None))
    );
}

#[tokio::test]
async fn unchanged_servers_try_the_incoming_server_only_with_the_password_the_host_holds() {
    let world = world().await;
    let current = current(&world, smtp_port(&world));
    let input = sent(&current, vec![]);
    let signed = done(edit(&world, mode(current, Some(STORED)), input).await);
    assert!(signed.credentials.is_empty(), "the sign-in files nothing");
    let attempts = world.imap.attempts();
    assert!(
        matches!(attempts.as_slice(), [a] if a.accepted && a.user == ADDRESS
            && is_password(&a.secret, STORED)),
        "imap: {attempts:?}"
    );
    assert!(world.smtp.attempts().is_empty(), "smtp was not changed");
}

#[tokio::test]
async fn an_smtp_only_change_logs_in_at_the_new_smtp_server() {
    let world = world().await;
    // Stored with an outgoing port nothing listens on; the person types the right one.
    let current = current(&world, 1);
    let fixed = vec![plain(
        FieldKind::OutgoingPort,
        &smtp_port(&world).to_string(),
    )];
    let input = sent(&current, fixed);
    let signed = done(edit(&world, mode(current, Some(STORED)), input).await);
    let attempts = world.smtp.attempts();
    assert!(
        matches!(attempts.as_slice(), [a] if a.accepted && a.user == ADDRESS
            && is_password(&a.secret, STORED)),
        "smtp: {attempts:?}"
    );
    let smtp = signed
        .endpoints
        .iter()
        .find(|e| e.family == Family::Smtp)
        .expect("smtp endpoint");
    assert_eq!(
        smtp.url.as_str(),
        format!("smtp://127.0.0.1:{}", smtp_port(&world))
    );
}

#[tokio::test]
async fn a_refusal_says_what_was_wrong_and_the_sign_in_may_be_started_again() {
    let world = world().await;
    let current = current(&world, smtp_port(&world));
    let held = || mode(current.clone(), Some(STORED));

    let wrong = vec![secret(FieldKind::Password, "wrong-pw-example")];
    let step = edit(&world, held(), sent(&current, wrong)).await;
    assert_eq!(step, SignInStep::Failed(SignInFault::Refused));

    let nobody = vec![plain(FieldKind::OutgoingPort, "1")];
    let step = edit(&world, held(), sent(&current, nobody)).await;
    assert_eq!(step, SignInStep::Failed(SignInFault::Unreachable));

    // The right password, typed: tried at both servers and handed back to the sheet.
    let right = vec![secret(FieldKind::Password, STORED)];
    done(edit(&world, held(), sent(&current, right)).await);
    assert!(
        world.smtp.attempts().iter().any(|a| a.accepted),
        "a typed password is tried at SMTP too"
    );
}

#[tokio::test]
async fn with_no_password_typed_and_none_held_the_password_is_what_is_missing() {
    let world = world().await;
    let current = current(&world, smtp_port(&world));
    let input = sent(&current, vec![]);
    let step = edit(&world, mode(current, None), input).await;
    assert_eq!(step, SignInStep::Failed(SignInFault::Refused));
    assert!(world.imap.attempts().is_empty(), "nothing was tried");
}

#[tokio::test]
async fn an_edit_never_sends_a_bearer_token_to_a_typed_host() {
    let world = world().await;
    let current = current(&world, smtp_port(&world));
    let typed = vec![
        secret(FieldKind::Password, STORED),
        plain(FieldKind::OutgoingPort, &smtp_port(&world).to_string()),
    ];
    done(
        edit(
            &world,
            mode(current.clone(), Some(STORED)),
            sent(&current, typed),
        )
        .await,
    );
    let wrong = vec![secret(FieldKind::Password, "wrong-pw-example")];
    edit(&world, mode(current.clone(), None), sent(&current, wrong)).await;
    let attempts: Vec<_> = [world.imap.attempts(), world.smtp.attempts()].concat();
    assert!(attempts.len() >= 3, "{attempts:?}");
    assert!(
        attempts
            .iter()
            .all(|a| matches!(a.secret, Secret::Password(_))),
        "{attempts:?}"
    );
}

#[tokio::test]
async fn another_protocol_than_the_accounts_is_not_this_edit() {
    let world = world().await;
    let current = current(&world, smtp_port(&world));
    let other = vec![plain(FieldKind::Protocol, "pop3")];
    let step = edit(
        &world,
        mode(current.clone(), Some(STORED)),
        sent(&current, other),
    )
    .await;
    assert_eq!(step, SignInStep::Failed(SignInFault::Unreadable));
    assert!(world.imap.attempts().is_empty());
}

#[tokio::test]
async fn a_lookup_whose_accounts_have_no_typed_servers_cannot_be_edited() {
    let spec = parse_provider(include_str!("../../../../providers/generic-dav.toml"))
        .expect("the shipped file");
    let provider = GenericProvider::<DavLookup>::new(
        spec,
        SharedHttp::new(Fakes::loopback()),
        FakeDns::new().unreachable(),
    );
    let current = Manual::Imap(MailServers {
        incoming: on_this_computer(1),
        outgoing: on_this_computer(2),
        login: None,
    });
    let mut signin = provider
        .sign_in(SignInStart::new(mode(current, Some(STORED))))
        .expect("sign-in");
    assert_eq!(
        signin.next(SignInInput::Start).await,
        SignInStep::Failed(SignInFault::Unreadable)
    );
    // The mail lookup is the one that can.
    const { assert!(<MailLookup as porter_family_generic::Lookup>::EDITS_SERVERS) };
}
