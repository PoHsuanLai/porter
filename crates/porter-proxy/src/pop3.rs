//! The POP3 relay (RFC 1939, 2449, 2595, 5034): it authenticates to the server and gives the app
//! a session that is already in the transaction state.
//!
//! 1. Dial; read the server's greeting (`+OK`); ask for `CAPA`.
//! 2. If the endpoint's TLS is `StartTls`, send `STLS` and have the host upgrade; ask for the
//!    capabilities again. A credential is never sent before the connection is secure.
//! 3. Authenticate by the credential: a password by `USER` and `PASS` (or `AUTH PLAIN` when the
//!    server offers `SASL PLAIN` and no `USER`); an access token by `AUTH XOAUTH2`.
//! 4. Tell the app `+OK porter relay ready`, then relay bytes both ways. The app's own `USER`,
//!    `PASS` and `APOP` are answered `+OK` here and never forwarded (a client that logs in
//!    anyway is told it succeeded); its `AUTH` and `STLS` are refused.

use crate::fault::RelayFault;
use porter_core::RelayAuth;

mod machine;

/// How the relay authenticates to a POP3 server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pop3Auth {
    /// `USER` then `PASS`.
    User,
    /// `AUTH PLAIN` with an initial response.
    Plain,
    /// `AUTH XOAUTH2` with an initial response.
    Xoauth2,
}

/// Whether `capabilities` (the lines of a `CAPA` reply) offer `SASL` with `mechanism`.
fn sasl(capabilities: &[String], mechanism: &str) -> bool {
    capabilities.iter().any(|line| {
        let mut words = line.split(' ');
        words.next().is_some_and(|w| w.eq_ignore_ascii_case("SASL"))
            && words.any(|m| m.eq_ignore_ascii_case(mechanism))
    })
}

impl Pop3Auth {
    /// The way to present `auth` to a server whose `CAPA` reply is `capabilities` (empty when it
    /// has no `CAPA`), or `Protocol` when it offers none: a token needs `SASL XOAUTH2`; a
    /// password goes by `USER`/`PASS`, the way every server takes it, unless the server offers
    /// `SASL PLAIN` and not `USER`.
    pub fn choose(capabilities: &[String], auth: &RelayAuth) -> Result<Self, RelayFault> {
        let user = capabilities.iter().any(|c| c.eq_ignore_ascii_case("USER"));
        match auth {
            RelayAuth::AccessToken(_) if sasl(capabilities, "XOAUTH2") => Ok(Pop3Auth::Xoauth2),
            RelayAuth::AccessToken(_) | RelayAuth::Anonymous => Err(RelayFault::Protocol),
            RelayAuth::Password(_) if !user && sasl(capabilities, "PLAIN") => Ok(Pop3Auth::Plain),
            RelayAuth::Password(_) => Ok(Pop3Auth::User),
        }
    }
}

/// What the app is greeted with once the relay has authenticated.
pub fn app_greeting() -> Vec<u8> {
    b"+OK porter relay ready\r\n".to_vec()
}

/// Where the relay stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pop3Phase {
    /// Waiting for the server's greeting.
    Greeting,
    /// Waiting for its capabilities.
    Capability,
    /// Waiting for the answer to `STLS`, then the upgrade.
    StartTls,
    /// Waiting for its capabilities after the upgrade.
    CapabilityAgain,
    /// `USER` is sent; waiting for the answer.
    User,
    /// `PASS` is sent; waiting for the answer.
    Pass,
    /// `AUTH` is sent; waiting for the answer (a `+` is the server's challenge, answered with an
    /// empty line).
    Authenticating,
    /// Relaying: the app's lines are commands, passed on except `USER`, `PASS`, `APOP`, `AUTH`
    /// and `STLS`, which are answered here; the server's bytes pass to the app.
    Relaying,
}

/// One POP3 relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pop3Relay {
    /// What it presents and where.
    pub plan: porter_core::RelayPlan,
    /// Where it stands.
    pub phase: Pop3Phase,
    /// Server bytes read and not yet a whole line, until the relay starts relaying.
    pub buffer: Vec<u8>,
    /// The lines of a `CAPA` reply read so far; `None` while its status line is awaited.
    pub reply: Option<Vec<String>>,
    /// The server's capabilities as last announced.
    pub capabilities: Vec<String>,
    /// App bytes not yet a whole command line (or sent before the relay was ready).
    pub early: Vec<u8>,
}

impl Pop3Relay {
    /// A relay at the start of a connection.
    pub fn new(plan: porter_core::RelayPlan) -> Self {
        Self {
            plan,
            phase: Pop3Phase::Greeting,
            buffer: Vec::new(),
            reply: None,
            capabilities: Vec::new(),
            early: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
