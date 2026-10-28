//! The wire protocol a relay speaks for an endpoint. A leaf: the endpoint (which names the
//! family) and the family (which names its protocol) both use it, and it names neither.

use porter_ids::UrlScheme;
use serde::{Deserialize, Serialize};

/// The wire protocol an authenticated relay speaks for an endpoint (`porter-proxy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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

impl EndpointProtocol {
    /// The protocol a URL scheme names; none for a scheme this build does not know.
    pub fn of_scheme(scheme: UrlScheme) -> Option<Self> {
        match scheme {
            UrlScheme::Http | UrlScheme::Https => Some(EndpointProtocol::Http),
            UrlScheme::Imap | UrlScheme::Imaps => Some(EndpointProtocol::Imap),
            UrlScheme::Smtp | UrlScheme::Smtps => Some(EndpointProtocol::Smtp),
            UrlScheme::Sieve | UrlScheme::Sieves => Some(EndpointProtocol::Sieve),
            UrlScheme::Pop3 | UrlScheme::Pop3s => Some(EndpointProtocol::Pop3),
            _ => None,
        }
    }
}
