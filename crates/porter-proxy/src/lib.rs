//! The authenticated relay behind `Tokens.OpenAuthenticated` (porter PLAN G2, D8): a password
//! never leaves accountd, so an app that needs one (generic IMAP and DAV, Nextcloud, iCloud)
//! gets a stream to a relay that authenticates for it.
//!
//! - **IMAP** (`imap`): the relay reads the server's greeting and capabilities, authenticates
//!   (`LOGIN`, `AUTHENTICATE PLAIN` or `XOAUTH2`) and gives the app a synthesized
//!   `* PREAUTH [CAPABILITY ...]` greeting, then relays bytes.
//! - **SMTP** (`smtp`): the relay does `EHLO`, `STARTTLS` and `AUTH`; the app sees a `220` and
//!   an `EHLO` reply without `AUTH` and `STARTTLS`, then relays bytes.
//! - **HTTP/1.1** (`http1`): WebDAV, CalDAV, CardDAV, OCS and JMAP. The app speaks plain HTTP on
//!   the stream; the relay adds `Authorization`, forwards over TLS to the endpoint's origin
//!   only, refuses any other origin and strips the app's own `Authorization`.
//!
//! - **ManageSieve** (`sieve`, RFC 5804): the relay does `STARTTLS` and `AUTHENTICATE`; the app
//!   sees the capability list without `SASL` and `STARTTLS`, then relays bytes.
//!
//! Each protocol is a pure machine (`step`: input in, effects out) over frozen types; the
//! `relay` drives one over two [`porter_core::stream::ByteStream`]s and a [`Connect`] the host
//! passes in (TCP, and the TLS check against system roots). TLS and certificate checks live in
//! the connector, never in the app. Nothing here reads the clock or the environment.

mod connect;
mod fault;
mod http1;
mod imap;
mod lines;
mod relay;
mod sieve;
mod smtp;
mod step;
#[cfg(test)]
mod testing;
#[cfg(feature = "tls")]
mod tls;
#[cfg(feature = "io")]
mod tokio_stream;

pub use connect::{Connect, ConnectFault};
pub use fault::RelayFault;
pub use http1::{ChunkParser, Chunked, HttpPhase, HttpRelay, rewrite_head};
pub use imap::{ImapAuth, ImapPhase, ImapRelay, app_capabilities, preauth_greeting};
pub use relay::relay;
pub use sieve::{SieveAuth, SievePhase, SieveRelay, app_greeting as sieve_greeting};
pub use smtp::{EhloReply, SmtpAuth, SmtpPhase, SmtpRelay, app_greeting};
pub use step::{Effect, Input, RelayEnd, Relaying, Side};
#[cfg(feature = "tls")]
pub use tls::{NetStream, RustlsConnect};
#[cfg(feature = "io")]
pub use tokio_stream::TokioStream;
