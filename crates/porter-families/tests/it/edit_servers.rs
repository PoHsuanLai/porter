//! Editing a generic mail account's servers (`SignInMode::EditServers`) against the fake IMAP
//! and SMTP servers: the incoming server is always logged in to, the outgoing one only when it,
//! the login name or the password changed, an empty password tries the stored one, and a refusal
//! names its field and lets the person send again.
#![cfg(feature = "generic")]

use crate::common::{Fakes, plain, secret};
use porter_core::sheet::{
    FieldAnswer, FieldKind, FieldProblem, Hop, MailServers, Manual, ProblemKind, Security,
    SignInInput, edit_form,
};
use porter_core::{AccountId, AccountLabel, Credential, SecretPurpose, SecretText, Tls};
use porter_fake::FakeAddress;
use porter_fake_servers::net::Bind;
use porter_fake_servers::{
    Accounts, FakeDns, FakeImap, FakeSmtp, MailHandle, Running, Secret, mailbox,
};
use porter_families::{GenericProvider, GenericSignIn};
use porter_http::SharedHttp;
use porter_provider::{
    Provider, SignIn, SignInMode, SignInStart, SignInStep, Signed, parse_provider,
};

const ADDRESS: &str = "ada@example.test";
const STORED: &str = "stored-pw";

struct World {
    imap: Running<MailHandle>,
    smtp: Running<MailHandle>,
    provider: GenericProvider,
}

async fn world() -> World {
    let accounts = Accounts::password(ADDRESS, STORED);
    let imap = FakeImap::bind(&Bind::Loopback, Tls::Plain, accounts.clone(), mailbox(1))
        .await
        .expect("imap");
    let smtp = FakeSmtp::bind(&Bind::Loopback, Tls::Plain, accounts)
        .await
        .expect("smtp");
    let (imap_handle, smtp_handle) = (imap.handle(), smtp.handle());
    let spec = parse_provider(include_str!("../../../../providers/generic-imap.toml"))
        .expect("the shipped file");
    World {
        imap: Running::spawn(imap, imap_handle),
        smtp: Running::spawn(smtp, smtp_handle),
        provider: GenericProvider::new(
            spec,
            SharedHttp::new(Fakes::loopback()),
            FakeDns::new().unreachable(),
        )
        .with_connect(porter_proxy::RustlsConnect::trusting([])),
    }
}

fn port(handle: &MailHandle) -> u16 {
    match handle.address() {
        FakeAddress::Loopback(port) => *port,
        other => panic!("a loopback fake: {other:?}"),
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
        incoming: on_this_computer(port(&world.imap)),
        outgoing: on_this_computer(smtp_port),
        login: None,
    })
}

fn edit(world: &World, current: Manual) -> GenericSignIn {
    let start = SignInStart::new(SignInMode::EditServers {
        account: AccountId::parse("ada").expect("id"),
        label: AccountLabel(ADDRESS.to_owned()),
        current,
        credentials: vec![(
            SecretPurpose::Password,
            Credential::Password(SecretText::new(STORED)),
        )],
    });
    world.provider.sign_in(start).expect("sign-in")
}

/// The edit form of `current` sent back as prefilled, with `changes` over it.
fn sent(current: &Manual, changes: Vec<FieldAnswer>) -> SignInInput {
    let mut answers: Vec<FieldAnswer> = edit_form(current, None)
        .iter()
        .filter(|f| changes.iter().all(|c| c.kind != f.kind))
        .map(|f| plain(f.kind, f.prefill.as_deref().unwrap_or("")))
        .collect();
    answers.extend(changes);
    SignInInput::Fields(answers)
}

fn done(step: SignInStep) -> Signed {
    match step {
        SignInStep::Done(signed) => signed,
        other => panic!("expected Done, got {other:?}"),
    }
}

fn refused(field: FieldKind) -> SignInStep {
    SignInStep::Refused(FieldProblem {
        field,
        problem: ProblemKind::Refused,
    })
}

#[tokio::test]
async fn unchanged_servers_try_the_incoming_server_only_with_the_stored_password() {
    let world = world().await;
    let current = current(&world, port(&world.smtp));
    let mut signin = edit(&world, current.clone());
    let signed = done(signin.next(sent(&current, vec![])).await);
    assert!(signed.credentials.is_empty(), "the stored password is kept");
    let attempts = world.imap.attempts();
    assert!(
        matches!(attempts.as_slice(), [a] if a.accepted && a.user == ADDRESS
            && a.secret == Secret::Password(STORED.to_owned())),
        "imap: {attempts:?}"
    );
    assert!(world.smtp.attempts().is_empty(), "smtp was not changed");
}

#[tokio::test]
async fn an_smtp_only_change_logs_in_at_the_new_smtp_server() {
    let world = world().await;
    // Stored with an outgoing port nothing listens on; the person types the right one.
    let current = current(&world, 1);
    let mut signin = edit(&world, current.clone());
    let fixed = vec![plain(
        FieldKind::OutgoingPort,
        &port(&world.smtp).to_string(),
    )];
    let signed = done(signin.next(sent(&current, fixed)).await);
    let attempts = world.smtp.attempts();
    assert!(
        matches!(attempts.as_slice(), [a] if a.accepted && a.user == ADDRESS
            && a.secret == Secret::Password(STORED.to_owned())),
        "smtp: {attempts:?}"
    );
    let smtp = signed
        .endpoints
        .iter()
        .find(|e| e.family == porter_core::Family::Smtp)
        .expect("smtp endpoint");
    assert_eq!(
        smtp.url.as_str(),
        format!("smtp://127.0.0.1:{}", port(&world.smtp))
    );
}

#[tokio::test]
async fn a_refusal_names_its_field_and_the_person_may_send_again() {
    let world = world().await;
    let current = current(&world, port(&world.smtp));
    let mut signin = edit(&world, current.clone());

    let wrong = vec![secret(FieldKind::Password, "wrong-pw")];
    let step = signin.next(sent(&current, wrong)).await;
    assert_eq!(step, refused(FieldKind::Password));

    let nobody = vec![plain(FieldKind::OutgoingPort, "1")];
    let step = signin.next(sent(&current, nobody)).await;
    assert_eq!(step, refused(FieldKind::OutgoingServer));

    // The right password, typed: tried at both servers and handed back to keep.
    let right = vec![secret(FieldKind::Password, STORED)];
    let signed = done(signin.next(sent(&current, right)).await);
    let [(SecretPurpose::Password, Credential::Password(password))] = signed.credentials.as_slice()
    else {
        panic!("one password: {:?}", signed.credentials);
    };
    assert_eq!(password.expose(), STORED);
    assert!(
        world.smtp.attempts().iter().any(|a| a.accepted),
        "a typed password is tried at SMTP too"
    );
}
