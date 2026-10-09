//! The SMTP relay against the fake SMTP server.

use crate::common;

use common::*;
use porter_core::{Family, Tls};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{Accounts, FakeSmtp, MailEvent, Mechanism, Secret};
use porter_proxy::RelayEnd;

fn accounts() -> Accounts {
    Accounts::password(USER, PASSWORD).with_bearer(TOKEN)
}

fn smtp_plan(tls: Tls, port: u16, auth: porter_core::RelayAuth) -> porter_core::RelayPlan {
    let scheme = match tls {
        Tls::Implicit => "smtps",
        _ => "smtp",
    };
    plan(
        Family::Smtp,
        &format!("{scheme}://127.0.0.1:{port}"),
        tls,
        auth,
    )
}

async fn send_mail(tls: Tls) {
    let fake = FakeSmtp::start(&Bind::Loopback, tls, accounts())
        .await
        .expect("smtp");
    let mut app = start(
        smtp_plan(tls, port(fake.address()), password()),
        trusting_fakes(),
    );
    assert_eq!(app.read_until("\r\n").await, "220 porter ESMTP ready\r\n");

    app.send("EHLO app.test\r\n").await;
    let ehlo = app.read_until("250 ").await;
    let ehlo = format!("{ehlo}{}", app.read_until("\r\n").await);
    assert!(ehlo.contains("8BITMIME") || ehlo.contains("SIZE"), "{ehlo}");
    assert!(
        !ehlo.contains("AUTH") && !ehlo.contains("STARTTLS"),
        "{ehlo}"
    );

    // The relay never forwards an app's AUTH or STARTTLS.
    app.send("AUTH PLAIN AGV2aWwAZ3Vlc3M=\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("503"));
    app.send("STARTTLS\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("503"));

    app.send("MAIL FROM:<alice@fake.test>\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("250"));
    app.send("RCPT TO:<bob@fake.test>\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("250"));
    app.send("DATA\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("354"));
    // A body line that looks like a command is only text.
    app.send("Subject: hi\r\n\r\nAUTH PLAIN nope\r\nSTARTTLS\r\n.\r\n")
        .await;
    assert!(app.read_until("\r\n").await.starts_with("250"));
    app.send("QUIT\r\n").await;
    assert!(app.read_until("\r\n").await.starts_with("221"));
    assert!(!app.everything().contains(PASSWORD));
    assert_eq!(app.finish().await, RelayEnd::Finished);

    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 1, "the relay's login, nothing the app sent");
    assert_eq!(attempts[0].secret, Secret::Password(PASSWORD.into()));
    assert_eq!(attempts[0].mechanism, Mechanism::Plain);
    let sent = fake.submissions();
    assert_eq!(sent.len(), 1);
    match &sent[0] {
        MailEvent::Submitted { from, to, data, .. } => {
            assert_eq!(from, "alice@fake.test");
            assert_eq!(to, &["bob@fake.test".to_owned()]);
            assert!(
                data.contains("AUTH PLAIN nope") && data.contains("STARTTLS"),
                "{data}"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// M1: mail goes out over a STARTTLS relay and an implicit TLS one. The logins are in login.rs.
#[tokio::test]
async fn mail_goes_out_through_a_relay_with_either_kind_of_tls() {
    for tls in [Tls::StartTls, Tls::Implicit] {
        send_mail(tls).await;
    }
}
