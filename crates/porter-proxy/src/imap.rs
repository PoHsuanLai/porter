//! The IMAP relay: it authenticates to the server and gives the app a session that is already
//! authenticated.
//!
//! 1. Dial; read the server's greeting. Without a `CAPABILITY` in it, ask for one.
//! 2. If the endpoint's TLS is `StartTls`, send `STARTTLS` and have the host upgrade; ask for
//!    the capabilities again (what a server offers changes after the upgrade).
//! 3. Authenticate by the credential: a password by `AUTHENTICATE PLAIN` when the server offers
//!    it, else `LOGIN` unless `LOGINDISABLED`; an access token by `AUTHENTICATE XOAUTH2`.
//! 4. Tell the app `* PREAUTH [CAPABILITY ...] ...`, with the server's capabilities less
//!    everything about authenticating or upgrading, then relay bytes both ways.

use crate::fault::RelayFault;
use porter_core::{RelayAuth, RelayPlan};

mod machine;

/// How the relay authenticates to an IMAP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImapAuth {
    /// `LOGIN user password`.
    Login,
    /// `AUTHENTICATE PLAIN`.
    Plain,
    /// `AUTHENTICATE XOAUTH2`.
    Xoauth2,
}

impl ImapAuth {
    /// The way to present `auth` to a server offering `capabilities` (upper case atoms, as the
    /// server wrote them), or `Protocol` when it offers none: a token needs `AUTH=XOAUTH2`, and
    /// a password needs `AUTH=PLAIN` or a `LOGIN` that is not disabled.
    pub fn choose(capabilities: &[String], auth: &RelayAuth) -> Result<Self, RelayFault> {
        let has = |atom: &str| capabilities.iter().any(|c| c.eq_ignore_ascii_case(atom));
        match auth {
            RelayAuth::AccessToken(_) if has("AUTH=XOAUTH2") => Ok(ImapAuth::Xoauth2),
            RelayAuth::AccessToken(_) => Err(RelayFault::Protocol),
            RelayAuth::Password(_) if has("AUTH=PLAIN") => Ok(ImapAuth::Plain),
            RelayAuth::Password(_) if !has("LOGINDISABLED") => Ok(ImapAuth::Login),
            RelayAuth::Password(_) => Err(RelayFault::Protocol),
        }
    }
}

/// The capabilities the app is told: the server's, less what concerns authenticating or
/// upgrading (`STARTTLS`, `LOGINDISABLED`, every `AUTH=` mechanism), since the app has no
/// credential to present and the connection is already as secure as the relay made it.
pub fn app_capabilities(server: &[String]) -> Vec<String> {
    server
        .iter()
        .filter(|c| {
            let c = c.to_ascii_uppercase();
            !(c == "STARTTLS" || c == "LOGINDISABLED" || c.starts_with("AUTH="))
        })
        .cloned()
        .collect()
}

/// The greeting the app sees: `* PREAUTH [CAPABILITY ...] porter relay ready`.
pub fn preauth_greeting(server_capabilities: &[String]) -> Vec<u8> {
    let caps = app_capabilities(server_capabilities).join(" ");
    format!("* PREAUTH [CAPABILITY {caps}] porter relay ready\r\n").into_bytes()
}

/// Where the relay stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImapPhase {
    /// Waiting for the server's greeting.
    Greeting,
    /// Waiting for its capabilities (before authenticating; after a `STARTTLS` upgrade too).
    Capability,
    /// Waiting for the answer to `STARTTLS`, then the upgrade.
    StartTls,
    /// Authenticating this way, waiting for the tagged answer (or, for `AUTHENTICATE`, the
    /// server's `+` that asks for the response).
    Authenticating(ImapAuth),
    /// The `AUTHENTICATE` response is sent; waiting for the tagged answer. A `+` now is the
    /// server's error challenge, answered with an empty line.
    Answering,
    /// Authenticated, and the tagged answer carried no capabilities: waiting for them.
    PostAuthCapability,
    /// Relaying bytes both ways.
    Relaying,
}

/// One IMAP relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapRelay {
    /// What it presents and where.
    pub plan: RelayPlan,
    /// Where it stands.
    pub phase: ImapPhase,
    /// Server bytes read and not yet a whole line, until the relay starts relaying.
    pub buffer: Vec<u8>,
    /// What the app sent before the relay was ready for it, passed on once it is.
    pub early: Vec<u8>,
    /// The server's capabilities as last announced.
    pub capabilities: Vec<String>,
}

impl ImapRelay {
    /// A relay at the start of a connection.
    pub fn new(plan: RelayPlan) -> Self {
        Self {
            plan,
            phase: ImapPhase::Greeting,
            buffer: Vec::new(),
            early: Vec::new(),
            capabilities: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
