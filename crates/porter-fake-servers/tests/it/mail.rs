//! The fake IMAP and SMTP servers driven with a minimal line client, over STARTTLS and implicit
//! TLS with the scratch CA.

use porter_core::Tls;
use porter_fake::{FakeAddress, FakeServer};
use porter_fake_servers::mail::{Lines, b64};
use porter_fake_servers::net::{Bind, Conn, dial};
use porter_fake_servers::{
    Accounts, FakeImap, FakeSmtp, MailEvent, Mechanism, Peer, Running, mailbox, tls,
};
use porter_fake_servers::{MailHandle, Secret};
use porter_provider::parse_provider;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::client::TlsStream;

type Plain = Lines<Conn>;
type Secured = Lines<TlsStream<Conn>>;

fn accounts() -> Accounts {
    Accounts::password("alice@fake.test", "hunter2").with_bearer("fake-access-9")
}

async fn secure(lines: Plain) -> Secured {
    let conn = lines.into_stream().expect("no bytes past STARTTLS");
    Lines::new(
        tls::connector()
            .connect(tls::server_name(), conn)
            .await
            .expect("handshake"),
    )
}

async fn implicit(address: &FakeAddress) -> Secured {
    let conn = dial(address).await.expect("dial");
    Lines::new(
        tls::connector()
            .connect(tls::server_name(), conn)
            .await
            .expect("handshake"),
    )
}

/// Reads until the tagged completion line; returns every line read.
async fn until_tag<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    tag: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let line = lines.read_line().await.expect("read").expect("line");
        let done = line.starts_with(&format!("{tag} "));
        out.push(line);
        if done {
            return out;
        }
    }
}

async fn command<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    tag: &str,
    text: &str,
) -> Vec<String> {
    lines.send(&format!("{tag} {text}")).await.expect("send");
    until_tag(lines, tag).await
}

fn last(lines: &[String]) -> &str {
    lines.last().map_or("", String::as_str)
}

async fn imap(tls_mode: Tls) -> Running<MailHandle> {
    FakeImap::start(&Bind::Loopback, tls_mode, accounts(), mailbox(3))
        .await
        .expect("imap")
}

fn addr(running: &FakeImap) -> FakeAddress {
    running.address().clone()
}

#[tokio::test]
async fn imap_starttls_refuses_login_in_the_clear_then_serves_a_session() {
    let fake = FakeImap::bind(&Bind::Loopback, Tls::StartTls, accounts(), mailbox(3))
        .await
        .expect("imap");
    let (address, handle) = (addr(&fake), fake.handle());
    let _running = Running::spawn(fake, handle.clone());

    let mut lines: Plain = Lines::new(dial(&address).await.expect("dial"));
    let greeting = lines.read_line().await.expect("read").expect("greeting");
    assert!(
        greeting.starts_with("* OK [CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED]"),
        "{greeting}"
    );
    let refused = command(&mut lines, "a1", "LOGIN alice@fake.test hunter2").await;
    assert!(last(&refused).starts_with("a1 NO [PRIVACYREQUIRED]"));
    assert_eq!(
        command(&mut lines, "a2", "STARTTLS").await,
        vec!["a2 OK Begin TLS negotiation now"]
    );

    let mut lines = secure(lines).await;
    let caps = command(&mut lines, "a3", "CAPABILITY").await;
    assert!(caps[0].contains("AUTH=PLAIN"), "{caps:?}");
    let wrong = command(&mut lines, "a4", "LOGIN alice@fake.test \"nope\"").await;
    assert!(last(&wrong).starts_with("a4 NO [AUTHENTICATIONFAILED]"));
    assert!(
        last(&command(&mut lines, "a5", "LOGIN alice@fake.test hunter2").await)
            .starts_with("a5 OK")
    );
    let selected = command(&mut lines, "a6", "SELECT INBOX").await;
    assert!(selected.contains(&"* 3 EXISTS".to_owned()) && last(&selected).contains("READ-WRITE"));
    let fetched = command(&mut lines, "a7", "FETCH 2 (UID FLAGS BODY.PEEK[])").await;
    assert!(
        fetched.iter().any(|l| l.contains("Subject: message 2")),
        "{fetched:?}"
    );
    let by_uid = command(&mut lines, "a8", "UID FETCH 1:* (FLAGS)").await;
    assert_eq!(by_uid.len(), 4);
    assert!(last(&command(&mut lines, "a9", "NOOP").await).starts_with("a9 OK"));
    assert!(
        command(&mut lines, "a10", "LOGOUT")
            .await
            .iter()
            .any(|l| l.starts_with("* BYE"))
    );

    // The refused clear-text LOGIN never reached a credential check; the rest did.
    let attempts = handle.attempts();
    assert_eq!(
        attempts.iter().map(|a| a.accepted).collect::<Vec<_>>(),
        vec![false, true]
    );
    assert!(
        attempts
            .iter()
            .all(|a| a.mechanism == Mechanism::Login && matches!(a.peer, Peer::Tcp(_)))
    );
    assert!(handle.events().contains(&MailEvent::StartTls { conn: 1 }));
}

#[tokio::test]
async fn imap_implicit_tls_takes_authenticate_plain_and_xoauth2() {
    let running = imap(Tls::Implicit).await;
    let address = running.address().clone();
    let mut lines = implicit(&address).await;
    assert!(
        lines
            .read_line()
            .await
            .expect("read")
            .expect("greeting")
            .contains("AUTH=PLAIN")
    );
    let plain = b64("\0alice@fake.test\0hunter2");
    assert!(
        last(&command(&mut lines, "b1", &format!("AUTHENTICATE PLAIN {plain}")).await)
            .starts_with("b1 OK")
    );
    assert!(command(&mut lines, "b2", "LOGOUT").await.len() >= 2);

    // Without an initial response the server prompts, and a bad password is refused.
    let mut lines = implicit(&address).await;
    lines.read_line().await.expect("read");
    lines.send("c1 AUTHENTICATE PLAIN").await.expect("send");
    assert_eq!(
        lines.read_line().await.expect("read").expect("prompt"),
        "+ "
    );
    lines
        .send(&b64("\0alice@fake.test\0wrong"))
        .await
        .expect("send");
    assert!(
        until_tag(&mut lines, "c1")
            .await
            .last()
            .expect("line")
            .starts_with("c1 NO [AUTHENTICATIONFAILED]")
    );

    let mut lines = implicit(&address).await;
    lines.read_line().await.expect("read");
    let xoauth = b64("user=alice@fake.test\u{1}auth=Bearer fake-access-9\u{1}\u{1}");
    assert!(
        last(&command(&mut lines, "d1", &format!("AUTHENTICATE XOAUTH2 {xoauth}")).await)
            .starts_with("d1 OK")
    );

    let attempts = running.attempts();
    assert_eq!(
        attempts
            .iter()
            .map(|a| (a.mechanism, a.accepted))
            .collect::<Vec<_>>(),
        vec![
            (Mechanism::Plain, true),
            (Mechanism::Plain, false),
            (Mechanism::Xoauth2, true)
        ]
    );
    assert_eq!(
        attempts[2].secret,
        Secret::Bearer("fake-access-9".to_owned())
    );
    // Three connections, so three distinct peers: a test tells the relay's from the app's by port.
    assert_eq!(running.peers().len(), 3);
}

#[tokio::test]
async fn imap_login_with_literals_and_quoted_strings() {
    let fake = FakeImap::bind(&Bind::Loopback, Tls::Plain, accounts(), mailbox(1))
        .await
        .expect("imap");
    let address = addr(&fake);
    let handle = fake.handle();
    let _running = Running::spawn(fake, handle.clone());
    let mut lines: Plain = Lines::new(dial(&address).await.expect("dial"));
    lines.read_line().await.expect("read");
    lines
        .send("e1 LOGIN \"alice@fake.test\" {7}")
        .await
        .expect("send");
    assert!(
        lines
            .read_line()
            .await
            .expect("read")
            .expect("continuation")
            .starts_with('+')
    );
    lines.send("hunter2").await.expect("send");
    assert!(
        until_tag(&mut lines, "e1")
            .await
            .last()
            .expect("line")
            .starts_with("e1 OK")
    );
    // Commands that need a login or a selected mailbox are refused before them.
    let mut other: Plain = Lines::new(dial(&address).await.expect("dial"));
    other.read_line().await.expect("read");
    assert!(last(&command(&mut other, "f1", "SELECT INBOX").await).starts_with("f1 BAD"));
}

async fn ehlo<S: AsyncRead + AsyncWrite + Unpin>(lines: &mut Lines<S>) -> Vec<String> {
    lines.send("EHLO client.test").await.expect("send");
    let mut out = Vec::new();
    loop {
        let line = lines.read_line().await.expect("read").expect("line");
        let more = line.starts_with("250-");
        out.push(line);
        if !more {
            return out;
        }
    }
}

async fn reply<S: AsyncRead + AsyncWrite + Unpin>(lines: &mut Lines<S>, send: &str) -> String {
    lines.send(send).await.expect("send");
    lines.read_line().await.expect("read").expect("reply")
}

#[tokio::test]
async fn smtp_starttls_authenticates_and_accepts_a_message() {
    let fake = FakeSmtp::bind(&Bind::Loopback, Tls::StartTls, accounts())
        .await
        .expect("smtp");
    let (address, handle) = (fake.address().clone(), fake.handle());
    let _running = Running::spawn(fake, handle.clone());

    let mut lines: Plain = Lines::new(dial(&address).await.expect("dial"));
    assert!(
        lines
            .read_line()
            .await
            .expect("read")
            .expect("greeting")
            .starts_with("220 ")
    );
    let caps = ehlo(&mut lines).await;
    assert!(
        caps.iter().any(|c| c.contains("STARTTLS")) && !caps.iter().any(|c| c.contains("AUTH")),
        "{caps:?}"
    );
    assert!(
        reply(&mut lines, "AUTH PLAIN AAAA")
            .await
            .starts_with("530")
    );
    assert!(reply(&mut lines, "STARTTLS").await.starts_with("220"));

    let mut lines = secure(lines).await;
    let caps = ehlo(&mut lines).await;
    assert!(
        caps.iter().any(|c| c.contains("AUTH PLAIN LOGIN")),
        "{caps:?}"
    );
    assert!(
        reply(&mut lines, "MAIL FROM:<alice@fake.test>")
            .await
            .starts_with("530")
    );

    // AUTH LOGIN with a wrong password, then PLAIN with the right one.
    assert!(
        reply(&mut lines, "AUTH LOGIN")
            .await
            .starts_with("334 VXNlcm5hbWU6")
    );
    assert!(
        reply(&mut lines, &b64("alice@fake.test"))
            .await
            .starts_with("334 UGFzc3dvcmQ6")
    );
    assert!(reply(&mut lines, &b64("wrong")).await.starts_with("535"));
    let plain = b64("\0alice@fake.test\0hunter2");
    assert!(
        reply(&mut lines, &format!("AUTH PLAIN {plain}"))
            .await
            .starts_with("235")
    );

    assert!(
        reply(&mut lines, "MAIL FROM:<alice@fake.test>")
            .await
            .starts_with("250")
    );
    assert!(
        reply(&mut lines, "RCPT TO:<bob@fake.test>")
            .await
            .starts_with("250")
    );
    assert!(reply(&mut lines, "DATA").await.starts_with("354"));
    lines
        .send("Subject: hi\r\n\r\n..dotted\r\nbody\r\n.")
        .await
        .expect("send");
    assert!(
        lines
            .read_line()
            .await
            .expect("read")
            .expect("reply")
            .starts_with("250")
    );
    assert!(reply(&mut lines, "QUIT").await.starts_with("221"));

    let sent = handle.submissions();
    assert_eq!(
        sent,
        vec![MailEvent::Submitted {
            conn: 1,
            from: "alice@fake.test".to_owned(),
            to: vec!["bob@fake.test".to_owned()],
            data: "Subject: hi\r\n\r\n.dotted\r\nbody\r\n".to_owned(),
        }]
    );
    let attempts = handle.attempts();
    assert_eq!(
        attempts
            .iter()
            .map(|a| (a.mechanism, a.accepted))
            .collect::<Vec<_>>(),
        vec![(Mechanism::Login, false), (Mechanism::Plain, true)]
    );
}

#[tokio::test]
async fn smtp_implicit_tls_greets_over_tls() {
    let fake = FakeSmtp::bind(&Bind::Loopback, Tls::Implicit, accounts())
        .await
        .expect("smtp");
    let (address, handle) = (fake.address().clone(), fake.handle());
    let _running = Running::spawn(fake, handle);
    let mut lines = implicit(&address).await;
    assert!(
        lines
            .read_line()
            .await
            .expect("read")
            .expect("greeting")
            .starts_with("220 ")
    );
    let caps = ehlo(&mut lines).await;
    assert!(
        caps.iter().any(|c| c.contains("AUTH")) && !caps.iter().any(|c| c.contains("STARTTLS"))
    );
}

#[tokio::test]
async fn a_client_that_does_not_trust_the_scratch_ca_fails_the_handshake() {
    let fake = FakeImap::bind(&Bind::Loopback, Tls::Implicit, accounts(), mailbox(1))
        .await
        .expect("imap");
    let (address, handle) = (addr(&fake), fake.handle());
    let _running = Running::spawn(fake, handle);
    let empty = std::sync::Arc::new(
        rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth(),
    );
    let conn = dial(&address).await.expect("dial");
    let attempt = tokio_rustls::TlsConnector::from(empty)
        .connect(tls::server_name(), conn)
        .await;
    assert!(attempt.is_err());
}

#[tokio::test]
async fn rewrite_points_mail_rows_at_the_fake() {
    let spec = parse_provider(include_str!(
        "../../../porter-fake/providers/fake-mail.toml"
    ))
    .expect("file");
    let fake = FakeImap::bind(&Bind::Loopback, Tls::Implicit, accounts(), mailbox(1))
        .await
        .expect("imap");
    let FakeAddress::Loopback(port) = fake.address().clone() else {
        unreachable!("bound on loopback")
    };
    let rewritten = fake.rewrite(&spec);
    let imap_rows: Vec<_> = rewritten
        .capabilities
        .iter()
        .filter(|r| r.family == porter_core::Family::Imap)
        .collect();
    assert!(!imap_rows.is_empty());
    assert!(
        imap_rows
            .iter()
            .all(|r| r.endpoint.as_ref().map(|e| e.0.clone())
                == Some(format!("imaps://127.0.0.1:{port}")))
    );
}

#[tokio::test]
async fn a_fake_can_listen_on_a_unix_socket_in_a_scratch_dir() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("imap-socket-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let fake = FakeImap::bind(
        &Bind::Socket(dir.clone()),
        Tls::Plain,
        accounts(),
        mailbox(1),
    )
    .await
    .expect("imap");
    let (address, handle) = (fake.address().clone(), fake.handle());
    assert_eq!(address, FakeAddress::Socket(dir.join("imap.sock")));
    let _running = Running::spawn(fake, handle.clone());
    let mut lines: Plain = Lines::new(dial(&address).await.expect("dial"));
    assert!(
        lines
            .read_line()
            .await
            .expect("read")
            .expect("greeting")
            .starts_with("* OK")
    );
    assert!(
        last(&command(&mut lines, "u1", "LOGIN alice@fake.test hunter2").await)
            .starts_with("u1 OK")
    );
    assert_eq!(handle.peers(), vec![Peer::Unix]);
    std::fs::remove_dir_all(&dir).expect("clean up");
}
