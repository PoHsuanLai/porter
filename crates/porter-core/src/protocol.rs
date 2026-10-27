//! The wire protocol a relay speaks for an endpoint. A leaf: the endpoint (which names the
//! family) and the family (which names its protocol) both use it, and it names neither.

use serde::{Deserialize, Serialize};

/// The wire protocol an authenticated relay speaks for an endpoint (`porter-proxy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointProtocol {
    /// IMAP: the relay authenticates and the app sees a `PREAUTH` greeting.
    Imap,
    /// SMTP submission: the relay does EHLO, STARTTLS and AUTH.
    Smtp,
    /// HTTP/1.1 (WebDAV, CalDAV, CardDAV, OCS, JMAP): the relay adds `Authorization`.
    Http,
    /// ManageSieve (RFC 5804): the relay does STARTTLS and AUTHENTICATE.
    Sieve,
    /// POP3 (RFC 1939): the relay does `STLS` and `USER`/`PASS` (or `AUTH`); the app sees a
    /// session that is already in the transaction state.
    Pop3,
}
