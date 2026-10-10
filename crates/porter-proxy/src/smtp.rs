//! The SMTP relay: `EHLO`, `STARTTLS` and `AUTH` happen between the relay and the server; the
//! app sees `220 porter ESMTP ready`, then an `EHLO` reply that offers no `AUTH` and no
//! `STARTTLS`, then a session that is already authenticated.

use crate::fault::RelayFault;
use porter_core::{RelayAuth, RelayPlan};

mod machine;

/// How the relay authenticates to an SMTP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmtpAuth {
    /// `AUTH PLAIN`.
    Plain,
    /// `AUTH XOAUTH2`.
    Xoauth2,
}

/// A server's reply to `EHLO`: its domain and the extensions it offers, one per line, without
/// the `250-` and `250 ` prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhloReply {
    /// The server's greeting line (its domain, usually followed by a note).
    pub domain: String,
    /// The extensions (`PIPELINING`, `SIZE 35882577`, `STARTTLS`, `AUTH PLAIN LOGIN`).
    pub extensions: Vec<String>,
}

fn body(line: &str, more: bool) -> Result<&str, RelayFault> {
    let prefix = if more { "250-" } else { "250 " };
    line.strip_prefix(prefix).ok_or(RelayFault::Protocol)
}

impl EhloReply {
    /// Reads a complete `250` reply: every line `250-...` but the last, which is `250 ...`.
    pub fn parse(reply: &[u8]) -> Result<Self, RelayFault> {
        let text = std::str::from_utf8(reply).map_err(|_| RelayFault::Protocol)?;
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        let (last, earlier) = lines.split_last().ok_or(RelayFault::Protocol)?;
        let mut texts = earlier
            .iter()
            .map(|line| body(line, true))
            .collect::<Result<Vec<_>, _>>()?;
        texts.push(body(last, false)?);
        let (domain, extensions) = texts.split_first().ok_or(RelayFault::Protocol)?;
        Ok(Self {
            domain: (*domain).to_owned(),
            extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
        })
    }

    /// Whether the server offers the extension `keyword` (compared without case).
    pub fn offers(&self, keyword: &str) -> bool {
        self.extensions.iter().any(|e| {
            e.split(' ')
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
        })
    }

    fn mechanisms(&self) -> Vec<String> {
        self.extensions
            .iter()
            .filter_map(|e| {
                let mut words = e.split(' ');
                let first = words.next()?;
                // `AUTH=PLAIN` is the legacy spelling some servers add beside `AUTH PLAIN`.
                first
                    .eq_ignore_ascii_case("AUTH")
                    .then(|| words.map(str::to_ascii_uppercase).collect::<Vec<_>>())
            })
            .flatten()
            .collect()
    }

    /// The reply the app sees: the same domain and extensions less `STARTTLS` and `AUTH`, the
    /// last line closing the reply.
    pub fn offered_to_app(&self) -> Vec<u8> {
        let kept: Vec<&String> = self
            .extensions
            .iter()
            .filter(|e| {
                let word = e.split(' ').next().unwrap_or_default();
                !(word.eq_ignore_ascii_case("STARTTLS")
                    || word.eq_ignore_ascii_case("AUTH")
                    || word.to_ascii_uppercase().starts_with("AUTH="))
            })
            .collect();
        let mut lines = vec![self.domain.as_str()];
        lines.extend(kept.iter().map(|e| e.as_str()));
        let last = lines.len() - 1;
        lines
            .iter()
            .enumerate()
            .map(|(at, line)| match at == last {
                true => format!("250 {line}\r\n"),
                false => format!("250-{line}\r\n"),
            })
            .collect::<String>()
            .into_bytes()
    }
}

impl SmtpAuth {
    /// The way to present `auth` to a server whose `EHLO` reply is `ehlo`, or `Protocol` when it
    /// offers none: a token needs `XOAUTH2`, a password needs `PLAIN`.
    pub fn choose(ehlo: &EhloReply, auth: &RelayAuth) -> Result<Self, RelayFault> {
        let mechanisms = ehlo.mechanisms();
        let offered = |name: &str| mechanisms.iter().any(|m| m == name);
        match auth {
            RelayAuth::AccessToken(_) if offered("XOAUTH2") => Ok(SmtpAuth::Xoauth2),
            RelayAuth::Password(_) if offered("PLAIN") => Ok(SmtpAuth::Plain),
            RelayAuth::AccessToken(_) | RelayAuth::Password(_) | RelayAuth::Anonymous => {
                Err(RelayFault::Protocol)
            }
            // a variant a newer porter adds: refused, never sent as anonymous
            _ => Err(RelayFault::Protocol),
        }
    }
}

/// What the app is greeted with before it sends `EHLO`.
pub fn app_greeting() -> Vec<u8> {
    b"220 porter ESMTP ready\r\n".to_vec()
}

/// Where the relay stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpPhase {
    /// Waiting for the server's `220`.
    Greeting,
    /// Waiting for its reply to our `EHLO`.
    Ehlo,
    /// Waiting for the answer to `STARTTLS`, then the upgrade.
    StartTls,
    /// Waiting for its reply to our `EHLO` after the upgrade.
    EhloAgain,
    /// Authenticating this way, waiting for the answer.
    Authenticating(SmtpAuth),
    /// Authenticated: waiting for the app's `EHLO`, which is answered from the server's reply.
    AppEhlo(EhloReply),
    /// Relaying: the app's lines are commands, passed on except `AUTH`, `STARTTLS` and a
    /// repeated `EHLO`, which are answered here; the server's bytes pass to the app.
    Relaying,
    /// Relaying a message body: the app's bytes pass on until the line holding only `.`.
    Data,
    /// Relaying a `BDAT` chunk of this many more bytes.
    Chunk {
        /// Bytes still to pass.
        remaining: u64,
    },
}

/// One SMTP relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpRelay {
    /// What it presents and where.
    pub plan: RelayPlan,
    /// Where it stands.
    pub phase: SmtpPhase,
    /// Server bytes read and not yet a whole line, until relaying starts.
    pub buffer: Vec<u8>,
    /// The lines of the server's reply read so far, with their `CRLF`.
    pub reply: Vec<u8>,
    /// The server's latest `EHLO` reply.
    pub ehlo: Option<EhloReply>,
    /// App bytes not yet a whole command line (or sent before the relay was ready).
    pub early: Vec<u8>,
    /// The start of the server's current line, to see a `354` (the go-ahead for a body).
    pub head: Vec<u8>,
    /// The last bytes of the body so far, to see its end across reads.
    pub tail: Vec<u8>,
}

impl SmtpRelay {
    /// A relay at the start of a connection.
    pub fn new(plan: RelayPlan) -> Self {
        Self {
            plan,
            phase: SmtpPhase::Greeting,
            buffer: Vec::new(),
            reply: Vec::new(),
            ehlo: None,
            early: Vec::new(),
            head: Vec::new(),
            tail: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
