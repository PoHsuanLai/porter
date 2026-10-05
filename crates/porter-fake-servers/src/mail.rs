//! What the fake IMAP and SMTP servers share: who may log in, the SASL payloads, the events they
//! record and the CRLF line reader.

use crate::net::Peer;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use porter_core::Tls;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// How a client proved itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// IMAP `LOGIN`, or SMTP `AUTH LOGIN`.
    Login,
    /// `AUTHENTICATE PLAIN`, or `AUTH PLAIN`.
    Plain,
    /// `XOAUTH2` with a bearer token.
    Xoauth2,
}

/// What a client presented: a password, or a bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    /// A password.
    Password(String),
    /// An OAuth access token.
    Bearer(String),
}

/// One login attempt, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The connection, numbered from 1 in arrival order.
    pub conn: u64,
    /// Who connected.
    pub peer: Peer,
    /// The mechanism used.
    pub mechanism: Mechanism,
    /// The login name.
    pub user: String,
    /// What was presented.
    pub secret: Secret,
    /// Whether the fake accepted it.
    pub accepted: bool,
}

/// One thing a mail fake saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailEvent {
    /// A connection arrived.
    Connected {
        /// Its number.
        conn: u64,
        /// Who connected.
        peer: Peer,
    },
    /// The client upgraded with STARTTLS.
    StartTls {
        /// The connection.
        conn: u64,
    },
    /// A login attempt.
    Auth(Attempt),
    /// SMTP: a message was submitted (after `DATA`).
    Submitted {
        /// The connection.
        conn: u64,
        /// `MAIL FROM`.
        from: String,
        /// Every `RCPT TO`.
        to: Vec<String>,
        /// The message as sent, dot-unstuffed.
        data: String,
    },
    /// IMAP: a command was received after login (`SELECT`, `FETCH`, ...), by verb.
    Command {
        /// The connection.
        conn: u64,
        /// The upper-case verb.
        verb: String,
    },
}

/// Who may log in, and with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accounts {
    /// The login name.
    pub user: String,
    /// The password.
    pub password: String,
    /// Access tokens accepted for `XOAUTH2`.
    pub bearer: Vec<String>,
}

impl Accounts {
    /// One account with a password and no bearer tokens.
    pub fn password(user: &str, password: &str) -> Self {
        Self {
            user: user.to_owned(),
            password: password.to_owned(),
            bearer: Vec::new(),
        }
    }

    /// Also accepts this bearer token for `XOAUTH2`.
    pub fn with_bearer(mut self, token: &str) -> Self {
        self.bearer.push(token.to_owned());
        self
    }

    /// Whether `user` with `secret` is let in.
    pub fn admits(&self, user: &str, secret: &Secret) -> bool {
        user == self.user
            && match secret {
                Secret::Password(p) => *p == self.password,
                Secret::Bearer(t) => self.bearer.contains(t),
            }
    }
}

/// `authzid NUL user NUL password`, base64: the `PLAIN` initial response.
pub fn decode_plain(b64: &str) -> Option<(String, Secret)> {
    let raw = String::from_utf8(STANDARD.decode(b64.trim()).ok()?).ok()?;
    let mut parts = raw.split('\0');
    let (_authzid, user, password) = (parts.next()?, parts.next()?, parts.next()?);
    Some((user.to_owned(), Secret::Password(password.to_owned())))
}

/// `user=U ^A auth=Bearer T ^A ^A`, base64: the `XOAUTH2` initial response.
pub fn decode_xoauth2(b64: &str) -> Option<(String, Secret)> {
    let raw = String::from_utf8(STANDARD.decode(b64.trim()).ok()?).ok()?;
    let user = raw.split('\u{1}').find_map(|p| p.strip_prefix("user="))?;
    let token = raw
        .split('\u{1}')
        .find_map(|p| p.strip_prefix("auth=Bearer "))?;
    Some((user.to_owned(), Secret::Bearer(token.to_owned())))
}

/// Base64 of text, for the `334` prompts and for tests building a `PLAIN` response.
pub fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

/// The decoded text of a base64 line (an `AUTH LOGIN` answer).
pub fn unb64(text: &str) -> Option<String> {
    String::from_utf8(STANDARD.decode(text.trim()).ok()?).ok()
}

/// Whether credentials may be sent now: never in the clear when the server wants STARTTLS first.
pub fn secure_enough(tls: Tls, active: bool) -> bool {
    !(tls == Tls::StartTls && !active)
}

/// A CRLF line reader and writer over one stream.
#[derive(Debug)]
pub struct Lines<S> {
    inner: BufReader<S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Lines<S> {
    /// Wraps a stream.
    pub fn new(stream: S) -> Self {
        Self {
            inner: BufReader::new(stream),
        }
    }

    /// The next line without its line end; `None` at the end of the stream.
    pub async fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        match self.inner.read_line(&mut line).await? {
            0 => Ok(None),
            _ => Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned())),
        }
    }

    /// Exactly `n` bytes (an IMAP literal).
    pub async fn read_exact(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0; n];
        self.inner.read_exact(&mut buf).await?;
        Ok(buf)
    }

    /// Writes `text` and a CRLF.
    pub async fn send(&mut self, text: &str) -> io::Result<()> {
        self.send_raw(&format!("{text}\r\n")).await
    }

    /// Writes `text` as it is.
    pub async fn send_raw(&mut self, text: &str) -> io::Result<()> {
        self.inner.get_mut().write_all(text.as_bytes()).await?;
        self.inner.get_mut().flush().await
    }

    /// The stream back, for a TLS upgrade; `None` when the client already sent bytes past the
    /// STARTTLS line (a command injection the upgrade must not swallow).
    pub fn into_stream(self) -> Option<S> {
        match self.inner.buffer().is_empty() {
            true => Some(self.inner.into_inner()),
            false => None,
        }
    }
}

/// The test's side of a running mail fake.
#[derive(Debug, Clone)]
pub struct MailHandle {
    pub(crate) events: crate::seen::Seen<MailEvent>,
    address: Option<porter_fake::FakeAddress>,
}

impl MailHandle {
    /// Where the server listens.
    pub fn address(&self) -> &porter_fake::FakeAddress {
        self.address
            .as_ref()
            .expect("a bound server's handle has its address")
    }

    /// Everything received, oldest first.
    pub fn events(&self) -> Vec<MailEvent> {
        self.events.all()
    }

    /// Every login attempt, accepted or not.
    pub fn attempts(&self) -> Vec<Attempt> {
        self.events
            .all()
            .into_iter()
            .filter_map(|e| match e {
                MailEvent::Auth(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// Every message submitted (SMTP).
    pub fn submissions(&self) -> Vec<MailEvent> {
        self.events
            .matching(|e| matches!(e, MailEvent::Submitted { .. }))
    }

    /// The peers that connected, in order.
    pub fn peers(&self) -> Vec<Peer> {
        self.events
            .all()
            .into_iter()
            .filter_map(|e| match e {
                MailEvent::Connected { peer, .. } => Some(peer),
                _ => None,
            })
            .collect()
    }
}

/// What every connection of one mail fake shares.
#[derive(Debug, Clone)]
pub(crate) struct MailCtx {
    pub accounts: Accounts,
    pub tls: Tls,
    pub handle: MailHandle,
    pub conns: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl MailCtx {
    pub fn new(accounts: Accounts, tls: Tls, address: porter_fake::FakeAddress) -> Self {
        Self {
            accounts,
            tls,
            handle: MailHandle {
                events: crate::seen::Seen::default(),
                address: Some(address),
            },
            conns: std::sync::Arc::default(),
        }
    }

    /// Numbers a new connection and records it.
    pub fn connected(&self, peer: Peer) -> u64 {
        let conn = self.conns.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.handle.events.push(MailEvent::Connected { conn, peer });
        conn
    }

    /// Checks a login, records the attempt, and says whether it was let in.
    pub fn attempt(
        &self,
        conn: u64,
        peer: Peer,
        mechanism: Mechanism,
        user: String,
        secret: Secret,
    ) -> bool {
        let accepted = self.accounts.admits(&user, &secret);
        self.handle.events.push(MailEvent::Auth(Attempt {
            conn,
            peer,
            mechanism,
            user,
            secret,
            accepted,
        }));
        accepted
    }
}
