//! A fake SMTP submission server: greeting, EHLO, STARTTLS or implicit TLS, AUTH PLAIN, LOGIN
//! and XOAUTH2, MAIL, RCPT, DATA, RSET, NOOP, QUIT. Mail is refused (530) before AUTH, a wrong
//! password is refused (535), and every accepted message is recorded.

use crate::mail::{
    Accounts, Lines, MailCtx, MailEvent, MailHandle, Mechanism, Secret, b64, decode_plain,
    decode_xoauth2, secure_enough, unb64,
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
use tokio::io::{AsyncRead, AsyncWrite};

/// The fake SMTP server, bound and ready to serve.
#[derive(Debug)]
pub struct FakeSmtp {
    listener: Listener,
    ctx: MailCtx,
}

impl FakeSmtp {
    /// Binds a server with this security, accepting `accounts`.
    pub async fn bind(bind: &Bind, tls: Tls, accounts: Accounts) -> io::Result<Self> {
        let listener = Listener::bind(bind, "smtp").await?;
        let ctx = MailCtx::new(accounts, tls, listener.address().clone());
        Ok(Self { listener, ctx })
    }

    /// The handle onto this server.
    pub fn handle(&self) -> MailHandle {
        self.ctx.handle.clone()
    }

    /// Binds and serves on a task.
    pub async fn start(
        bind: &Bind,
        tls: Tls,
        accounts: Accounts,
    ) -> io::Result<Running<MailHandle>> {
        let fake = Self::bind(bind, tls, accounts).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl FakeServer for FakeSmtp {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::Smtp
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        let scheme = match self.ctx.tls {
            Tls::Implicit => "smtps",
            Tls::StartTls | Tls::Plain => "smtp",
        };
        point(
            spec,
            Family::Smtp,
            &format!(
                "{scheme}://127.0.0.1:{}",
                crate::net::port_of(self.address())
            ),
        )
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let (listener, ctx) = (self.listener, self.ctx);
        async move {
            let acceptor = tls::acceptor();
            while let Ok((conn, peer)) = listener.accept().await {
                let (ctx, acceptor) = (ctx.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let n = ctx.connected(peer);
                    // A client that drops mid-session is not the fake's failure.
                    let _ = run(conn, &ctx, acceptor, n, peer).await;
                });
            }
        }
    }
}

async fn run(
    conn: crate::net::Conn,
    ctx: &MailCtx,
    acceptor: tokio_rustls::TlsAcceptor,
    n: u64,
    peer: Peer,
) -> io::Result<()> {
    match ctx.tls {
        Tls::Implicit => {
            let stream = acceptor.accept(conn).await?;
            phase(stream, ctx, n, peer, Entry::Greet { secure: true })
                .await
                .map(|_| ())
        }
        Tls::Plain => phase(conn, ctx, n, peer, Entry::Greet { secure: false })
            .await
            .map(|_| ()),
        Tls::StartTls => match phase(conn, ctx, n, peer, Entry::Greet { secure: false }).await? {
            Some(conn) => {
                let stream = acceptor.accept(conn).await?;
                phase(stream, ctx, n, peer, Entry::Resume).await.map(|_| ())
            }
            None => Ok(()),
        },
    }
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    Greet { secure: bool },
    Resume,
}

/// The mail transaction in progress.
#[derive(Debug, Default)]
struct Txn {
    from: Option<String>,
    to: Vec<String>,
}

fn address_of(rest: &str) -> String {
    let inner = rest.split_once(':').map_or(rest, |(_, a)| a);
    inner
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['<', '>'])
        .to_owned()
}

async fn phase<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    ctx: &MailCtx,
    n: u64,
    peer: Peer,
    entry: Entry,
) -> io::Result<Option<S>> {
    let mut lines = Lines::new(stream);
    let secure = matches!(entry, Entry::Resume | Entry::Greet { secure: true });
    if matches!(entry, Entry::Greet { .. }) {
        lines.send("220 fake.test ESMTP ready").await?;
    }
    let mut authed = false;
    let mut txn = Txn::default();
    while let Some(line) = lines.read_line().await? {
        let (verb, rest) = line.split_once(' ').unwrap_or((&line, ""));
        match verb.to_ascii_uppercase().as_str() {
            "EHLO" | "HELO" => ehlo(&mut lines, ctx, secure).await?,
            "STARTTLS" if ctx.tls == Tls::StartTls && !secure => {
                lines.send("220 ready to start TLS").await?;
                ctx.handle.events.push(MailEvent::StartTls { conn: n });
                return Ok(lines.into_stream());
            }
            "STARTTLS" => lines.send("503 STARTTLS not available").await?,
            "AUTH" if !secure_enough(ctx.tls, secure) => {
                lines
                    .send("530 Must issue a STARTTLS command first")
                    .await?
            }
            "AUTH" => authed = auth(&mut lines, ctx, (n, peer), rest).await? || authed,
            "MAIL" if !authed => lines.send("530 Authentication required").await?,
            "MAIL" => {
                txn = Txn {
                    from: Some(address_of(rest)),
                    to: Vec::new(),
                };
                lines.send("250 OK").await?;
            }
            "RCPT" if txn.from.is_some() => {
                txn.to.push(address_of(rest));
                lines.send("250 OK").await?;
            }
            "RCPT" => lines.send("503 MAIL first").await?,
            "DATA" if txn.from.is_some() && !txn.to.is_empty() => {
                lines.send("354 End data with <CR><LF>.<CR><LF>").await?;
                let data = read_data(&mut lines).await?;
                let done = std::mem::take(&mut txn);
                ctx.handle.events.push(MailEvent::Submitted {
                    conn: n,
                    from: done.from.unwrap_or_default(),
                    to: done.to,
                    data,
                });
                lines.send("250 OK queued").await?;
            }
            "DATA" => lines.send("503 MAIL and RCPT first").await?,
            "RSET" => {
                txn = Txn::default();
                lines.send("250 OK").await?;
            }
            "NOOP" => lines.send("250 OK").await?,
            "QUIT" => {
                lines.send("221 bye").await?;
                return Ok(None);
            }
            _ => lines.send("500 unrecognized command").await?,
        }
    }
    Ok(None)
}

async fn ehlo<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    ctx: &MailCtx,
    secure: bool,
) -> io::Result<()> {
    let mut caps = vec!["fake.test".to_owned()];
    if ctx.tls == Tls::StartTls && !secure {
        caps.push("STARTTLS".to_owned());
    }
    if secure_enough(ctx.tls, secure) {
        caps.push("AUTH PLAIN LOGIN XOAUTH2".to_owned());
    }
    caps.extend(["8BITMIME".to_owned(), "SIZE 10485760".to_owned()]);
    let last = caps.len() - 1;
    for (i, cap) in caps.iter().enumerate() {
        lines
            .send(&format!("250{}{cap}", if i == last { ' ' } else { '-' }))
            .await?;
    }
    Ok(())
}

/// Runs one AUTH exchange; whether it succeeded.
async fn auth<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    ctx: &MailCtx,
    (n, peer): (u64, Peer),
    rest: &str,
) -> io::Result<bool> {
    let mut words = rest.split_whitespace();
    let mechanism = words.next().unwrap_or("").to_ascii_uppercase();
    let initial = words.next().map(str::to_owned);
    let credentials = match mechanism.as_str() {
        "PLAIN" => sasl(lines, initial, decode_plain)
            .await?
            .map(|(u, s)| (Mechanism::Plain, u, s)),
        "XOAUTH2" => sasl(lines, initial, decode_xoauth2)
            .await?
            .map(|(u, s)| (Mechanism::Xoauth2, u, s)),
        "LOGIN" => login(lines, initial).await?,
        _ => {
            lines.send("504 unrecognized authentication type").await?;
            return Ok(false);
        }
    };
    let admitted =
        credentials.is_some_and(|(mech, user, secret)| ctx.attempt(n, peer, mech, user, secret));
    match admitted {
        true => lines.send("235 2.7.0 Authentication successful").await?,
        false => {
            lines
                .send("535 5.7.8 Authentication credentials invalid")
                .await?
        }
    }
    Ok(admitted)
}

async fn sasl<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    initial: Option<String>,
    decode: fn(&str) -> Option<(String, Secret)>,
) -> io::Result<Option<(String, Secret)>> {
    let response = match initial {
        Some(r) => r,
        None => {
            lines.send("334 ").await?;
            lines.read_line().await?.unwrap_or_default()
        }
    };
    Ok(decode(&response))
}

async fn login<S: AsyncRead + AsyncWrite + Unpin>(
    lines: &mut Lines<S>,
    initial: Option<String>,
) -> io::Result<Option<(Mechanism, String, Secret)>> {
    let user = match initial {
        Some(u) => u,
        None => {
            lines.send(&format!("334 {}", b64("Username:"))).await?;
            lines.read_line().await?.unwrap_or_default()
        }
    };
    lines.send(&format!("334 {}", b64("Password:"))).await?;
    let password = lines.read_line().await?.unwrap_or_default();
    Ok(unb64(&user)
        .zip(unb64(&password))
        .map(|(u, p)| (Mechanism::Login, u, Secret::Password(p))))
}

/// The message after `DATA`, up to the lone dot, with dot-stuffing undone.
async fn read_data<S: AsyncRead + AsyncWrite + Unpin>(lines: &mut Lines<S>) -> io::Result<String> {
    let mut data = String::new();
    while let Some(line) = lines.read_line().await? {
        if line == "." {
            break;
        }
        data.push_str(
            line.strip_prefix('.')
                .filter(|_| line.starts_with(".."))
                .unwrap_or(&line),
        );
        data.push_str("\r\n");
    }
    Ok(data)
}
