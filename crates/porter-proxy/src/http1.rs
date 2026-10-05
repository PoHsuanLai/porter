//! The HTTP/1.1 relay (WebDAV, CalDAV, CardDAV, OCS, JMAP). The app speaks plain HTTP on its
//! stream; the relay forwards each request over TLS to the endpoint's origin and nowhere else:
//!
//! - the request target is origin-form, or absolute-form with the endpoint's origin (rewritten
//!   to origin-form); any other origin, and a `Host` that is not the endpoint's, is refused
//!   with `ForeignOrigin` and the connection ends;
//! - every `Authorization` and `Proxy-Authorization` the app sent is dropped, and the relay adds
//!   its own (`Basic` from the endpoint's login and the password, `Bearer` for an access
//!   token);
//! - a request whose framing is ambiguous (both `Content-Length` and `Transfer-Encoding`, a
//!   repeated `Content-Length`) is refused with `Protocol`, never forwarded;
//! - bodies, chunked or by length, and responses pass through unchanged.

use crate::fault::RelayFault;
use crate::step::{Effect, Input, Relaying};
use porter_core::RelayPlan;

/// Where the relay stands in the current request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpPhase {
    /// Reading a request head (up to the blank line).
    Head,
    /// Passing a body of this many more bytes.
    Body {
        /// Bytes still to pass.
        remaining: u64,
    },
    /// Passing a chunked body up to its last chunk.
    Chunked,
}

/// One HTTP relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRelay {
    /// What it presents and where.
    pub plan: RelayPlan,
    /// Where it stands.
    pub phase: HttpPhase,
    /// Head bytes read so far, until the blank line.
    pub head: Vec<u8>,
}

impl HttpRelay {
    /// A relay at the start of a connection.
    pub fn new(plan: RelayPlan) -> Self {
        Self {
            plan,
            phase: HttpPhase::Head,
            head: Vec::new(),
        }
    }
}

/// The head the server is sent for the app's `head` (request line and headers, through the blank
/// line): the target and `Host` checked against the plan's origin, the app's credentials
/// dropped and the relay's added; or why it is refused.
pub fn rewrite_head(head: &[u8], plan: &RelayPlan) -> Result<Vec<u8>, RelayFault> {
    let _ = (head, plan);
    todo!(
        "parse the request line and headers, check the target and Host against \
         `plan.endpoint.url.origin()`, drop Authorization and Proxy-Authorization, refuse \
         ambiguous framing, add Basic or Bearer from `plan.auth`"
    )
}

impl Relaying for HttpRelay {
    fn step(self, input: Input) -> (Self, Vec<Effect>) {
        let _ = input;
        todo!(
            "buffer a head until the blank line, send `rewrite_head` to the server, pass the \
             body by `phase`, pass responses to the app unchanged"
        )
    }
}
