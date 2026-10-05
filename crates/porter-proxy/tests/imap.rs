//! The IMAP relay against the fake IMAP server: the app sees a PREAUTH session, the fake's
//! logins come only from the relay, and no byte the app received holds the password.

mod common;

use common::*;
use porter_core::{Family, Tls};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{Accounts, FakeImap, MailEvent, Mechanism, Secret, mailbox};
use porter_proxy::{RelayEnd, RelayFault, RustlsConnect};

fn accounts() -> Accounts {
    Accounts::password(USER, PASSWORD).with_bearer(TOKEN)
}

async fn imap(
    tls: Tls,
) -> (
    porter_fake_servers::Running<porter_fake_servers::MailHandle>,
    u16,
) {
    let running = FakeImap::start(&Bind::Loopback, tls, accounts(), mailbox(3))
        .await
        .expect("imap");
    let port = port(running.address());
    (running, port)
}

fn imap_plan(tls: Tls, port: u16, auth: porter_core::RelayAuth) -> porter_core::RelayPlan {
    let scheme = match tls {
        Tls::Implicit => "imaps",
        _ => "imap",
    };
    plan(
        Family::Imap,
        &format!("{scheme}://127.0.0.1:{port}"),
        tls,
        auth,
    )
}

async fn session_works(tls: Tls) {
    let (fake, port) = imap(tls).await;
    let mut app = start(imap_plan(tls, port, password()), trusting_fakes());
    let greeting = app.read_until("\r\n").await;
    assert!(
        greeting.starts_with("* PREAUTH [CAPABILITY IMAP4rev1 UIDPLUS]"),
        "{greeting}"
    );
    for hidden in ["STARTTLS", "AUTH=", "LOGINDISABLED"] {
        assert!(!greeting.contains(hidden), "{greeting}");
    }
    app.send("a1 SELECT INBOX\r\n").await;
    let select = app.read_until("a1 OK").await;
    assert!(select.contains("* 3 EXISTS"), "{select}");
    app.send("a2 FETCH 2 (BODY[])\r\n").await;
    let fetch = app.read_until("a2 OK").await;
    assert!(fetch.contains("Subject: message 2"), "{fetch}");
    app.send("a3 LOGOUT\r\n").await;
    app.read_until("a3 OK").await;
    assert_eq!(app.everything().matches(PASSWORD).count(), 0);
    let all = app.everything();
    assert_eq!(app.finish().await, RelayEnd::Finished);

    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1, "one login, made by the relay");
    assert_eq!(attempts[0].user, USER);
    assert_eq!(attempts[0].secret, Secret::Password(PASSWORD.into()));
    assert_eq!(attempts[0].mechanism, Mechanism::Plain);
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
    assert!(verbs.contains(&"SELECT".to_owned()) && verbs.contains(&"FETCH".to_owned()));
    assert!(!all.contains(PASSWORD));
}

#[tokio::test]
async fn a_starttls_server_gives_the_app_a_preauth_session() {
    session_works(Tls::StartTls).await;
}

#[tokio::test]
async fn an_implicit_tls_server_gives_the_app_a_preauth_session() {
    session_works(Tls::Implicit).await;
}

#[tokio::test]
async fn a_wrong_password_is_a_typed_refusal_and_no_session() {
    let (fake, port) = imap(Tls::StartTls).await;
    let wrong = porter_core::RelayAuth::Password(porter_core::SecretText::new("not-it"));
    let mut app = start(imap_plan(Tls::StartTls, port, wrong), trusting_fakes());
    let seen = app.read_to_end().await;
    assert_eq!(seen, "", "the app is told nothing: no greeting, no session");
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Refused));
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].accepted);
}

#[tokio::test]
async fn an_access_token_authenticates_with_xoauth2() {
    let (fake, port) = imap(Tls::Implicit).await;
    let mut app = start(imap_plan(Tls::Implicit, port, token()), trusting_fakes());
    let greeting = app.read_until("\r\n").await;
    assert!(greeting.starts_with("* PREAUTH"), "{greeting}");
    assert!(!app.everything().contains(TOKEN));
    assert_eq!(fake.attempts()[0].mechanism, Mechanism::Xoauth2);
    assert_eq!(fake.attempts()[0].secret, Secret::Bearer(TOKEN.into()));
    app.finish().await;
}

#[tokio::test]
async fn a_certificate_the_connector_does_not_trust_ends_the_relay_before_any_login() {
    let (fake, port) = imap(Tls::Implicit).await;
    let nobody = RustlsConnect::trusting([]);
    let mut app = start(imap_plan(Tls::Implicit, port, password()), nobody);
    assert_eq!(app.read_to_end().await, "");
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Tls));
    assert!(fake.attempts().is_empty(), "no credential went anywhere");

    // The same through a STARTTLS upgrade.
    let (fake, port) = imap(Tls::StartTls).await;
    let app = start(
        imap_plan(Tls::StartTls, port, password()),
        RustlsConnect::trusting([]),
    );
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Tls));
    assert!(fake.attempts().is_empty());
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let (fake, port) = imap(Tls::StartTls).await;
    drop(fake);
    // The fake's task is aborted, but its listener may linger: use a port nobody holds.
    let closed = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let _ = port;
    let app = start(
        imap_plan(Tls::StartTls, closed, password()),
        trusting_fakes(),
    );
    assert_eq!(app.ended().await, RelayEnd::Failed(RelayFault::Unreachable));
}
