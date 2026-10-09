//! The POP3 relay against the fake POP3 server: the app sees a session already in the
//! transaction state, the fake's logins come only from the relay, and no byte the app received
//! holds the password.

use crate::common;

use common::*;
use porter_core::{Family, Tls};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{Accounts, FakePop3, MailEvent, Mechanism, Secret, mailbox};
use porter_proxy::RelayEnd;

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

/// M1: the same session over a STARTTLS server and an implicit TLS one. The logins are in login.rs.
#[tokio::test]
async fn a_server_with_either_kind_of_tls_gives_the_app_a_transaction_session() {
    for tls in [Tls::StartTls, Tls::Implicit] {
        session_works(tls).await;
    }
}
