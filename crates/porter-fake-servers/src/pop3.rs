//! A fake POP3 server: greeting, CAPA, STLS or implicit TLS, USER/PASS, AUTH PLAIN and XOAUTH2,
//! and enough of STAT, LIST, RETR, DELE, NOOP and QUIT for a relay test to see a session work.
//! It refuses a wrong password, and refuses USER, PASS and AUTH before TLS when it advertises
//! STLS. Every login attempt and every command after login is recorded on the mail handle.

use crate::imap::Message;
use crate::mail::{
    Accounts, Lines, MailCtx, MailEvent, MailHandle, Mechanism, Secret, decode_plain,
    decode_xoauth2, secure_enough,
};
use crate::net::{Bind, Listener, Peer};
use crate::seen::Running;
use crate::shipped::point;
use crate::tls;
use porter_core::{Family, Tls};
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use std::future::Future;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Debug, Clone)]
struct Shared {
    ctx: MailCtx,
    maildrop: Arc<Vec<Message>>,
}

/// The fake POP3 server, bound and ready to serve.
#[derive(Debug)]
pub struct FakePop3 {
    listener: Listener,
    shared: Shared,
}

impl FakePop3 {
    /// Binds a server with this security, accepting `accounts`, holding `maildrop`.
    pub async fn bind(
        bind: &Bind,
        tls: Tls,
        accounts: Accounts,
        maildrop: Vec<Message>,
    ) -> io::Result<Self> {
        let listener = Listener::bind(bind, "pop3").await?;
        let ctx = MailCtx::new(accounts, tls, listener.address().clone());
        Ok(Self {
            listener,
            shared: Shared {
                ctx,
                maildrop: Arc::new(maildrop),
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
        maildrop: Vec<Message>,
    ) -> io::Result<Running<MailHandle>> {
        let fake = Self::bind(bind, tls, accounts, maildrop).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl FakeServer for FakePop3 {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Pop3
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        let scheme = match self.shared.ctx.tls {
            Tls::Implicit => "pop3s",
            Tls::StartTls | Tls::Plain => "pop3",
        };
        point(
            spec,
            Family::Pop3,
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
            phase(stream, shared, (n, peer), Entry::Greet { secure: true })
                .await
                .map(|_| ())
        }
        Tls::Plain => phase(conn, shared, (n, peer), Entry::Greet { secure: false })
            .await
            .map(|_| ()),
        Tls::StartTls => {
            match phase(conn, shared, (n, peer), Entry::Greet { secure: false }).await? {
                Some(conn) => {
                    let stream = acceptor.accept(conn).await?;
                    phase(stream, shared, (n, peer), Entry::Resume)
                        .await
                        .map(|_| ())
                }
                None => Ok(()),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    Greet { secure: bool },
    Resume,
}

/// Where a session stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    /// Not logged in; a `USER` was given, if any.
    Authorization(Option<String>),
    /// Logged in.
    Transaction,
}

fn capabilities(shared: &Shared, secure: bool) -> Vec<&'static str> {
    let mut caps = vec!["TOP", "UIDL", "RESP-CODES", "PIPELINING"];
    if shared.ctx.tls == Tls::StartTls && !secure {
        caps.push("STLS");
    }
    if secure_enough(shared.ctx.tls, secure) {
        caps.extend(["USER", "SASL PLAIN XOAUTH2"]);
    }
    caps
}

async fn phase<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    shared: &Shared,
    (n, peer): (u64, Peer),
    entry: Entry,
) -> io::Result<Option<S>> {
    let ctx = &shared.ctx;
    let mut lines = Lines::new(stream);
    let secure = matches!(entry, Entry::Resume | Entry::Greet { secure: true });
    if matches!(entry, Entry::Greet { .. }) {
        lines.send("+OK fake POP3 ready").await?;
    }
    let mut stage = Stage::Authorization(None);
    let mut deleted: Vec<u32> = Vec::new();
    while let Some(line) = lines.read_line().await? {
        let (verb, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let verb = verb.to_ascii_uppercase();
        let login_ok = secure_enough(ctx.tls, secure);
        if stage == Stage::Transaction {
            ctx.handle.events.push(MailEvent::Command {
                conn: n,
                verb: verb.clone(),
            });
        }
        match (verb.as_str(), &stage) {
            ("CAPA", _) => {
                lines.send("+OK capabilities follow").await?;
                for cap in capabilities(shared, secure) {
                    lines.send(cap).await?;
                }
                lines.send(".").await?;
            }
            ("STLS", Stage::Authorization(_)) if ctx.tls == Tls::StartTls && !secure => {
                lines.send("+OK begin TLS negotiation").await?;
                ctx.handle.events.push(MailEvent::StartTls { conn: n });
                return Ok(lines.into_stream());
            }
            ("STLS", _) => lines.send("-ERR STLS not available").await?,
            ("USER" | "PASS" | "AUTH", Stage::Authorization(_)) if !login_ok => {
                lines.send("-ERR [AUTH] Plaintext login disabled").await?
            }
            ("USER", Stage::Authorization(_)) => {
                stage = Stage::Authorization(Some(rest.to_owned()));
                lines.send("+OK send PASS").await?;
            }
            ("PASS", Stage::Authorization(Some(user))) => {
                let secret = Secret::Password(rest.to_owned());
                let admitted = ctx.attempt(n, peer, Mechanism::Login, user.clone(), secret);
                match admitted {
                    true => {
                        stage = Stage::Transaction;
                        lines.send("+OK maildrop locked and ready").await?;
                    }
                    false => {
                        stage = Stage::Authorization(None);
                        lines.send("-ERR [AUTH] invalid credentials").await?;
                    }
                }
            }
            ("PASS", _) => lines.send("-ERR USER first").await?,
            ("AUTH", Stage::Authorization(_)) => {
                match auth(&mut lines, ctx, (n, peer), rest).await? {
                    true => stage = Stage::Transaction,
                    false => lines.send("-ERR [AUTH] invalid credentials").await?,
                }
                if stage == Stage::Transaction {
                    lines.send("+OK maildrop locked and ready").await?;
                }
            }
            ("QUIT", _) => {
                lines.send("+OK bye").await?;
                return Ok(None);
            }
            ("NOOP", Stage::Transaction) => lines.send("+OK").await?,
            ("STAT", Stage::Transaction) => {
                let live = shared.maildrop.iter().filter(|m| !deleted.contains(&m.uid));
                let (count, octets) = live.fold((0, 0), |(c, o), m| (c + 1, o + m.raw.len()));
                lines.send(&format!("+OK {count} {octets}")).await?;
            }
            ("LIST", Stage::Transaction) => {
                lines.send("+OK scan listing follows").await?;
                for m in shared.maildrop.iter().filter(|m| !deleted.contains(&m.uid)) {
                    lines.send(&format!("{} {}", m.uid, m.raw.len())).await?;
                }
                lines.send(".").await?;
            }
            ("RETR", Stage::Transaction) => {
                let wanted = rest.trim().parse::<u32>().ok();
                match shared
                    .maildrop
                    .iter()
                    .find(|m| Some(m.uid) == wanted && !deleted.contains(&m.uid))
                {
                    Some(m) => {
                        lines.send(&format!("+OK {} octets", m.raw.len())).await?;
                        for text in m.raw.split("\r\n") {
                            // Dot-stuffing: a line starting with a dot gets another.
                            match text.starts_with('.') {
                                true => lines.send(&format!(".{text}")).await?,
                                false => lines.send(text).await?,
                            }
                        }
                        lines.send(".").await?;
                    }
                    None => lines.send("-ERR no such message").await?,
                }
            }
            ("DELE", Stage::Transaction) => {
                match rest.trim().parse::<u32>() {
                    Ok(uid) if shared.maildrop.iter().any(|m| m.uid == uid) => {
                        deleted.push(uid);
                        lines.send("+OK message deleted").await?;
                    }
                    _ => lines.send("-ERR no such message").await?,
                };
            }
            _ => lines.send("-ERR unrecognized command").await?,
        }
    }
    Ok(None)
}

/// Runs one AUTH exchange (the final `+OK` or `-ERR` is the caller's); whether it succeeded.
async fn auth<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    ctx: &MailCtx,
    (n, peer): (u64, Peer),
    rest: &str,
) -> io::Result<bool> {
    let mut words = rest.split_whitespace();
    let mechanism = words.next().unwrap_or("").to_ascii_uppercase();
    let initial = words.next().map(str::to_owned);
    let (mech, decode): (Mechanism, fn(&str) -> Option<(String, Secret)>) = match mechanism.as_str()
    {
        "PLAIN" => (Mechanism::Plain, decode_plain),
        "XOAUTH2" => (Mechanism::Xoauth2, decode_xoauth2),
        _ => return Ok(false),
    };
    let response = match initial {
        Some(r) => r,
        None => {
            lines.send("+ ").await?;
            lines.read_line().await?.unwrap_or_default()
        }
    };
    Ok(decode(&response).is_some_and(|(user, secret)| ctx.attempt(n, peer, mech, user, secret)))
}
