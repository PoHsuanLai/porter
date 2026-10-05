//! A fake IMAP server: greeting, CAPABILITY, STARTTLS or implicit TLS, LOGIN, AUTHENTICATE
//! PLAIN and XOAUTH2, and enough of SELECT, FETCH, UID FETCH, LIST, NOOP and LOGOUT for a relay
//! test to see a session work. It refuses a wrong password and refuses LOGIN before TLS when it
//! advertises STARTTLS.

mod commands;

use crate::mail::{
    Accounts, Lines, MailCtx, MailEvent, MailHandle, Mechanism, decode_plain, decode_xoauth2,
    secure_enough,
};
use crate::net::{Bind, Listener, Peer};
use crate::seen::Running;
use crate::shipped::point;
use crate::tls;
use commands::{fetch, read_args, select};
use porter_core::{Family, Tls};
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use std::future::Future;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

/// One message in the fake mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Its UID (and sequence number: UIDs run 1..n here).
    pub uid: u32,
    /// The RFC 822 text.
    pub raw: String,
}

/// A mailbox of `count` short messages.
pub fn mailbox(count: u32) -> Vec<Message> {
    (1..=count)
        .map(|uid| Message {
            uid,
            raw: format!("From: a@fake.test\r\nTo: b@fake.test\r\nSubject: message {uid}\r\n\r\nbody {uid}\r\n"),
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Shared {
    ctx: MailCtx,
    inbox: Arc<Vec<Message>>,
}

/// The fake IMAP server, bound and ready to serve.
#[derive(Debug)]
pub struct FakeImap {
    listener: Listener,
    shared: Shared,
}

impl FakeImap {
    /// Binds a server with this security, accepting `accounts`, holding `inbox`.
    pub async fn bind(
        bind: &Bind,
        tls: Tls,
        accounts: Accounts,
        inbox: Vec<Message>,
    ) -> io::Result<Self> {
        let listener = Listener::bind(bind, "imap").await?;
        let ctx = MailCtx::new(accounts, tls, listener.address().clone());
        Ok(Self {
            listener,
            shared: Shared {
                ctx,
                inbox: Arc::new(inbox),
            },
        })
    }

    /// The handle onto this server.
    pub fn handle(&self) -> MailHandle {
        self.shared.ctx.handle.clone()
    }

    /// Binds and serves on a task.
    pub async fn start(
        bind: &Bind,
        tls: Tls,
        accounts: Accounts,
        inbox: Vec<Message>,
    ) -> io::Result<Running<MailHandle>> {
        let fake = Self::bind(bind, tls, accounts, inbox).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl FakeServer for FakeImap {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Imap
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        let scheme = match self.shared.ctx.tls {
            Tls::Implicit => "imaps",
            Tls::StartTls | Tls::Plain => "imap",
        };
        point(
            spec,
            Family::Imap,
            &format!(
                "{scheme}://127.0.0.1:{}",
                crate::net::port_of(self.address())
            ),
        )
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let (listener, shared) = (self.listener, self.shared);
        async move {
            let acceptor = tls::acceptor();
            while let Ok((conn, peer)) = listener.accept().await {
                let (shared, acceptor) = (shared.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let n = shared.ctx.connected(peer);
                    // A client that drops mid-session is not the fake's failure.
                    let _ = run(conn, &shared, acceptor, n, peer).await;
                });
            }
        }
    }
}

async fn run(
    conn: crate::net::Conn,
    shared: &Shared,
    acceptor: tokio_rustls::TlsAcceptor,
    n: u64,
    peer: Peer,
) -> io::Result<()> {
    match shared.ctx.tls {
        Tls::Implicit => {
            let stream = acceptor.accept(conn).await?;
            phase(stream, shared, n, peer, Entry::Greet { secure: true })
                .await
                .map(|_| ())
        }
        Tls::Plain => phase(conn, shared, n, peer, Entry::Greet { secure: false })
            .await
            .map(|_| ()),
        Tls::StartTls => {
            match phase(conn, shared, n, peer, Entry::Greet { secure: false }).await? {
                Some(conn) => {
                    let stream = acceptor.accept(conn).await?;
                    phase(stream, shared, n, peer, Entry::Resume)
                        .await
                        .map(|_| ())
                }
                None => Ok(()),
            }
        }
    }
}

/// How a phase of a session begins.
#[derive(Debug, Clone, Copy)]
enum Entry {
    /// A new connection: send the greeting.
    Greet { secure: bool },
    /// After STARTTLS: the channel is secure and no greeting is sent.
    Resume,
}

/// Where a session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    NotAuthenticated,
    Authenticated,
    Selected,
}

fn capability(shared: &Shared, secure: bool, stage: Stage) -> String {
    let tls = shared.ctx.tls;
    match (stage, secure_enough(tls, secure)) {
        (Stage::NotAuthenticated, false) => "IMAP4rev1 STARTTLS LOGINDISABLED".to_owned(),
        (Stage::NotAuthenticated, true) => "IMAP4rev1 AUTH=PLAIN AUTH=XOAUTH2".to_owned(),
        _ => "IMAP4rev1 UIDPLUS".to_owned(),
    }
}

/// Runs one phase of a session; returns the stream when the client asked for STARTTLS and the
/// fake agreed.
async fn phase<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    shared: &Shared,
    n: u64,
    peer: Peer,
    entry: Entry,
) -> io::Result<Option<S>> {
    let mut lines = Lines::new(stream);
    let secure = matches!(entry, Entry::Resume | Entry::Greet { secure: true });
    if matches!(entry, Entry::Greet { .. }) {
        let caps = capability(shared, secure, Stage::NotAuthenticated);
        lines
            .send(&format!("* OK [CAPABILITY {caps}] fake imap ready"))
            .await?;
    }
    let mut stage = Stage::NotAuthenticated;
    while let Some(first) = lines.read_line().await? {
        let args = read_args(&mut lines, &first).await?;
        let (Some(tag), Some(verb)) = (
            args.first().cloned(),
            args.get(1).map(|v| v.to_ascii_uppercase()),
        ) else {
            lines.send("* BAD empty command").await?;
            continue;
        };
        let rest = &args[2..];
        if stage != Stage::NotAuthenticated {
            shared.ctx.handle.events.push(MailEvent::Command {
                conn: n,
                verb: verb.clone(),
            });
        }
        match verb.as_str() {
            "CAPABILITY" => {
                lines
                    .send(&format!(
                        "* CAPABILITY {}",
                        capability(shared, secure, stage)
                    ))
                    .await?;
                lines
                    .send(&format!("{tag} OK CAPABILITY completed"))
                    .await?;
            }
            "NOOP" => lines.send(&format!("{tag} OK NOOP completed")).await?,
            "LOGOUT" => {
                lines.send("* BYE fake imap closing").await?;
                lines.send(&format!("{tag} OK LOGOUT completed")).await?;
                return Ok(None);
            }
            "STARTTLS" if shared.ctx.tls == Tls::StartTls && !secure => {
                lines
                    .send(&format!("{tag} OK Begin TLS negotiation now"))
                    .await?;
                shared
                    .ctx
                    .handle
                    .events
                    .push(MailEvent::StartTls { conn: n });
                return Ok(lines.into_stream());
            }
            "LOGIN" => {
                let ok = secure_enough(shared.ctx.tls, secure) && stage == Stage::NotAuthenticated;
                match (ok, rest) {
                    (false, _) => {
                        lines
                            .send(&format!("{tag} NO [PRIVACYREQUIRED] LOGIN is disabled"))
                            .await?
                    }
                    (true, [user, password]) => {
                        let secret = crate::mail::Secret::Password(password.clone());
                        let admitted =
                            shared
                                .ctx
                                .attempt(n, peer, Mechanism::Login, user.clone(), secret);
                        stage = reply_auth(&mut lines, &tag, admitted, stage).await?;
                    }
                    (true, _) => {
                        lines
                            .send(&format!("{tag} BAD LOGIN needs a user and a password"))
                            .await?
                    }
                }
            }
            "AUTHENTICATE" => {
                stage =
                    authenticate(&mut lines, shared, (n, peer, secure), &tag, rest, stage).await?;
            }
            "SELECT" | "EXAMINE" if stage != Stage::NotAuthenticated => {
                match rest.first().map(|m| m.eq_ignore_ascii_case("INBOX")) {
                    Some(true) => {
                        select(&mut lines, shared, &tag, &verb).await?;
                        stage = Stage::Selected;
                    }
                    _ => {
                        lines
                            .send(&format!("{tag} NO [NONEXISTENT] no such mailbox"))
                            .await?
                    }
                }
            }
            "LIST" if stage != Stage::NotAuthenticated => {
                lines
                    .send("* LIST (\\HasNoChildren) \"/\" \"INBOX\"")
                    .await?;
                lines.send(&format!("{tag} OK LIST completed")).await?;
            }
            "FETCH" | "UID" if stage == Stage::Selected => {
                fetch(&mut lines, shared, &tag, &verb, rest).await?
            }
            "CLOSE" if stage == Stage::Selected => {
                stage = Stage::Authenticated;
                lines.send(&format!("{tag} OK CLOSE completed")).await?;
            }
            _ if stage == Stage::NotAuthenticated => {
                lines
                    .send(&format!("{tag} BAD command not valid before login"))
                    .await?;
            }
            _ => lines.send(&format!("{tag} BAD unknown command")).await?,
        }
    }
    Ok(None)
}

async fn reply_auth<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    tag: &str,
    admitted: bool,
    stage: Stage,
) -> io::Result<Stage> {
    match admitted {
        true => {
            lines
                .send(&format!(
                    "{tag} OK [CAPABILITY IMAP4rev1 UIDPLUS] logged in"
                ))
                .await?;
            Ok(Stage::Authenticated)
        }
        false => {
            lines
                .send(&format!(
                    "{tag} NO [AUTHENTICATIONFAILED] invalid credentials"
                ))
                .await?;
            Ok(stage)
        }
    }
}

async fn authenticate<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    shared: &Shared,
    (n, peer, secure): (u64, Peer, bool),
    tag: &str,
    rest: &[String],
    stage: Stage,
) -> io::Result<Stage> {
    if !secure_enough(shared.ctx.tls, secure) || stage != Stage::NotAuthenticated {
        lines
            .send(&format!(
                "{tag} NO [PRIVACYREQUIRED] AUTHENTICATE is disabled"
            ))
            .await?;
        return Ok(stage);
    }
    let Some(mechanism) = rest.first().map(|m| m.to_ascii_uppercase()) else {
        lines
            .send(&format!("{tag} BAD AUTHENTICATE needs a mechanism"))
            .await?;
        return Ok(stage);
    };
    let (mech, decode): (_, fn(&str) -> _) = match mechanism.as_str() {
        "PLAIN" => (Mechanism::Plain, decode_plain),
        "XOAUTH2" => (Mechanism::Xoauth2, decode_xoauth2),
        _ => {
            lines
                .send(&format!("{tag} NO unsupported mechanism"))
                .await?;
            return Ok(stage);
        }
    };
    let response = match rest.get(1) {
        Some(initial) => initial.clone(),
        None => {
            lines.send("+ ").await?;
            lines.read_line().await?.unwrap_or_default()
        }
    };
    if response == "*" {
        lines
            .send(&format!("{tag} BAD authentication cancelled"))
            .await?;
        return Ok(stage);
    }
    let admitted = match decode(&response) {
        Some((user, secret)) => shared.ctx.attempt(n, peer, mech, user, secret),
        None => false,
    };
    reply_auth(lines, tag, admitted, stage).await
}
