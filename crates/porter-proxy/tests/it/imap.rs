//! The IMAP relay against the fake IMAP server: the app sees a PREAUTH session, the fake's
//! logins come only from the relay, and no byte the app received holds the password.

use crate::common;

use common::*;
use porter_core::{Family, Tls};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{Accounts, FakeImap, MailEvent, Mechanism, Secret, mailbox};
use porter_proxy::RelayEnd;

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

/// M1: the same session over a STARTTLS server and an implicit TLS one. The logins are in login.rs.
#[tokio::test]
async fn a_server_with_either_kind_of_tls_gives_the_app_a_preauth_session() {
    for tls in [Tls::StartTls, Tls::Implicit] {
        session_works(tls).await;
    }
}
