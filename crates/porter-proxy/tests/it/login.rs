//! What the relay does with the login, the same for every mail protocol (M1: one table per
//! behaviour, one row per protocol). The sessions after the login are in each protocol's own
//! module.

use crate::common;

use common::*;
use porter_core::{Family, RelayAuth, RelayPlan, SecretText, Tls};
use porter_fake_servers::net::Bind;
use porter_fake_servers::{
    Accounts, FakeImap, FakePop3, FakeSmtp, MailHandle, Mechanism, Running, Secret, mailbox,
};
use porter_proxy::{RelayEnd, RelayFault, RustlsConnect};

#[derive(Clone, Copy, Debug)]
enum Proto {
    Imap,
    Pop3,
    Smtp,
}

const PROTOCOLS: [Proto; 3] = [Proto::Imap, Proto::Pop3, Proto::Smtp];

impl Proto {
    fn family(self) -> Family {
        match self {
            Proto::Imap => Family::Imap,
            Proto::Pop3 => Family::Pop3,
            Proto::Smtp => Family::Smtp,
        }
    }

    fn scheme(self, tls: Tls) -> &'static str {
        match (self, tls) {
            (Proto::Imap, Tls::Implicit) => "imaps",
            (Proto::Imap, _) => "imap",
            (Proto::Pop3, Tls::Implicit) => "pop3s",
            (Proto::Pop3, _) => "pop3",
            (Proto::Smtp, Tls::Implicit) => "smtps",
            (Proto::Smtp, _) => "smtp",
        }
    }

    /// What the app is greeted with once the relay has logged in.
    fn ready(self) -> &'static str {
        match self {
            Proto::Imap => "* PREAUTH",
            Proto::Pop3 => "+OK porter relay ready\r\n",
            Proto::Smtp => "220 porter ESMTP ready\r\n",
        }
    }

    async fn fake(self, tls: Tls) -> (Running<MailHandle>, u16) {
        let accounts = Accounts::password(USER, PASSWORD).with_bearer(TOKEN);
        let running = match self {
            Proto::Imap => FakeImap::start(&Bind::Loopback, tls, accounts, mailbox(3)).await,
            Proto::Pop3 => FakePop3::start(&Bind::Loopback, tls, accounts, mailbox(3)).await,
            Proto::Smtp => FakeSmtp::start(&Bind::Loopback, tls, accounts).await,
        }
        .expect("the fake starts");
        let port = port(running.address());
        (running, port)
    }

    fn plan(self, tls: Tls, port: u16, auth: RelayAuth) -> RelayPlan {
        plan(
            self.family(),
            &format!("{}://127.0.0.1:{port}", self.scheme(tls)),
            tls,
            auth,
        )
    }
}

#[tokio::test]
async fn a_wrong_password_is_a_typed_refusal_and_no_session() {
    for proto in PROTOCOLS {
        let (fake, port) = proto.fake(Tls::StartTls).await;
        let wrong = RelayAuth::Password(SecretText::new("not-it"));
        let mut app = start(proto.plan(Tls::StartTls, port, wrong), trusting_fakes());
        let seen = app.read_to_end().await;
        assert_eq!(
            seen, "",
            "{proto:?}: the app is told nothing: no greeting, no session"
        );
        assert_eq!(
            app.ended().await,
            RelayEnd::Failed(RelayFault::Refused),
            "{proto:?}"
        );
        let attempts = fake.attempts();
        assert_eq!(attempts.len(), 1, "{proto:?}");
        assert!(!attempts[0].accepted, "{proto:?}");
    }
}

#[tokio::test]
async fn an_access_token_authenticates_with_xoauth2() {
    for (proto, tls) in [
        (Proto::Imap, Tls::Implicit),
        (Proto::Pop3, Tls::Implicit),
        (Proto::Smtp, Tls::StartTls),
    ] {
        let (fake, port) = proto.fake(tls).await;
        let mut app = start(proto.plan(tls, port, token()), trusting_fakes());
        let greeting = app.read_until("\r\n").await;
        assert!(greeting.starts_with(proto.ready()), "{proto:?}: {greeting}");
        assert!(!app.everything().contains(TOKEN), "{proto:?}");
        assert_eq!(
            fake.attempts()[0].mechanism,
            Mechanism::Xoauth2,
            "{proto:?}"
        );
        assert_eq!(
            fake.attempts()[0].secret,
            Secret::Bearer(TOKEN.into()),
            "{proto:?}"
        );
        app.finish().await;
    }
}

#[tokio::test]
async fn a_certificate_the_connector_does_not_trust_ends_the_relay_before_any_login() {
    for proto in PROTOCOLS {
        // The same through an implicit TLS connection and a STARTTLS upgrade.
        for tls in [Tls::Implicit, Tls::StartTls] {
            let (fake, port) = proto.fake(tls).await;
            let mut app = start(
                proto.plan(tls, port, password()),
                RustlsConnect::trusting([]),
            );
            assert_eq!(app.read_to_end().await, "", "{proto:?} {tls:?}");
            assert_eq!(
                app.ended().await,
                RelayEnd::Failed(RelayFault::Tls),
                "{proto:?} {tls:?}"
            );
            assert!(
                fake.attempts().is_empty(),
                "{proto:?} {tls:?}: no credential went anywhere"
            );
        }
    }
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    for proto in PROTOCOLS {
        // A port nobody holds.
        let closed = {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let app = start(
            proto.plan(Tls::StartTls, closed, password()),
            trusting_fakes(),
        );
        assert_eq!(
            app.ended().await,
            RelayEnd::Failed(RelayFault::Unreachable),
            "{proto:?}"
        );
    }
}
