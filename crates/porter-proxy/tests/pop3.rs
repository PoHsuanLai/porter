//! The POP3 relay against the fake POP3 server: the app sees a session already in the
//! transaction state, the fake's logins come only from the relay, and no byte the app received
//! holds the password.

mod common;

use common::*;
use porter_core::{Family, Tls};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{Accounts, FakePop3, MailEvent, Mechanism, Secret, mailbox};
use porter_proxy::{RelayEnd, RelayFault, RustlsConnect};

fn accounts() -> Accounts {
    Accounts::password(USER, PASSWORD).with_bearer(TOKEN)
}

async fn pop3(
    tls: Tls,
) -> (
    porter_fake_servers::Running<porter_fake_servers::MailHandle>,
    u16,
) {
    let running = FakePop3::start(&Bind::Loopback, tls, accounts(), mailbox(3))
        .await
        .expect("pop3");
    let port = port(running.address());
    (running, port)
}

fn pop3_plan(tls: Tls, port: u16, auth: porter_core::RelayAuth) -> porter_core::RelayPlan {
    let scheme = match tls {
        Tls::Implicit => "pop3s",
        _ => "pop3",
    };
    plan(
        Family::Pop3,
        &format!("{scheme}://127.0.0.1:{port}"),
        tls,
        auth,
    )
}

async fn session_works(tls: Tls) {
    let (fake, port) = pop3(tls).await;
    let mut app = start(pop3_plan(tls, port, password()), trusting_fakes());
    assert_eq!(app.read_until("\r\n").await, "+OK porter relay ready\r\n");

    // A client that logs in anyway is told it worked, and the fake never hears it.
    app.send("USER someone\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("+OK"));
    app.send("PASS not-the-password\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("+OK"));

    app.send("STAT\r\n").await;
    let stat = app.read_until("\r\n").await;
    assert!(stat.starts_with("+OK 3 "), "{stat}");
    app.send("RETR 2\r\n").await;
    let retr = app.read_until("\r\n.\r\n").await;
    assert!(retr.contains("Subject: message 2"), "{retr}");
    app.send("STLS\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("-ERR"));
    app.send("QUIT\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("+OK"));
    let all = app.everything();
    assert_eq!(app.finish().await, RelayEnd::Finished);
    assert!(!all.contains(PASSWORD));

    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1, "one login, made by the relay");
    assert_eq!(attempts[0].user, USER);
    assert_eq!(attempts[0].secret, Secret::Password(PASSWORD.into()));
    assert_eq!(attempts[0].mechanism, Mechanism::Login);
    assert!(attempts[0].accepted);
    let connections = fake
        .events()
        .iter()
        .filter(|e| matches!(e, MailEvent::Connected { .. }))
        .count();
    assert_eq!(connections, 1, "the app never connected to the fake itself");
    let verbs: Vec<_> = fake
        .events()
        .into_iter()
        .filter_map(|e| match e {
            MailEvent::Command { verb, .. } => Some(verb),
            _ => None,
        })
        .collect();
    assert!(verbs.contains(&"STAT".to_owned()) && verbs.contains(&"RETR".to_owned()));
    assert!(
        !verbs.contains(&"USER".to_owned()) && !verbs.contains(&"STLS".to_owned()),
        "what the relay answered was not forwarded: {verbs:?}"
    );
}

#[tokio::test]
async fn a_starttls_server_gives_the_app_a_transaction_session() {
    session_works(Tls::StartTls).await;
}

#[tokio::test]
async fn an_implicit_tls_server_gives_the_app_a_transaction_session() {
    session_works(Tls::Implicit).await;
}

#[tokio::test]
async fn a_wrong_password_is_a_typed_refusal_and_no_session() {
    let (fake, port) = pop3(Tls::StartTls).await;
    let wrong = porter_core::RelayAuth::Password(porter_core::SecretText::new("not-it"));
    let mut app = start(pop3_plan(Tls::StartTls, port, wrong), trusting_fakes());
    assert_eq!(app.read_to_end().await, "", "no greeting, no session");
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Refused));
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].accepted);
}

#[tokio::test]
async fn an_access_token_authenticates_with_xoauth2() {
    let (fake, port) = pop3(Tls::Implicit).await;
    let mut app = start(pop3_plan(Tls::Implicit, port, token()), trusting_fakes());
    assert_eq!(app.read_until("\r\n").await, "+OK porter relay ready\r\n");
    assert!(!app.everything().contains(TOKEN));
    assert_eq!(fake.attempts()[0].mechanism, Mechanism::Xoauth2);
    assert_eq!(fake.attempts()[0].secret, Secret::Bearer(TOKEN.into()));
    app.finish().await;
}

#[tokio::test]
async fn a_certificate_the_connector_does_not_trust_ends_the_relay_before_any_login() {
    let (fake, port) = pop3(Tls::Implicit).await;
    let mut app = start(
        pop3_plan(Tls::Implicit, port, password()),
        RustlsConnect::trusting([]),
    );
    assert_eq!(app.read_to_end().await, "");
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Tls));
    assert!(fake.attempts().is_empty(), "no credential went anywhere");

    let (fake, port) = pop3(Tls::StartTls).await;
    let app = start(
        pop3_plan(Tls::StartTls, port, password()),
        RustlsConnect::trusting([]),
    );
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Tls));
    assert!(fake.attempts().is_empty());
}
