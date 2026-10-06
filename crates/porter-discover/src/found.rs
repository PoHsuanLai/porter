//! What a discovery step returns.

use porter_core::{Claim, ServiceEndpoint};

/// Where a finding came from, for the review step ("from the provider's autoconfig", "via MX").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A provider file.
    Provider,
    /// The domain's autoconfig, or Thunderbird's database of them.
    Autoconfig,
    /// SRV records.
    Srv,
    /// An MX lead to a known provider.
    Mx,
    /// A `.well-known` URL.
    WellKnown,
    /// The JMAP session resource.
    JmapSession,
    /// Nextcloud's OCS capabilities.
    Ocs,
    /// A probe of local ports.
    Probe,
}

/// What discovery found for an address or a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The servers.
    pub endpoints: Vec<ServiceEndpoint>,
    /// What the account can do, at `Discovered` or `Probed` provenance.
    pub claims: Vec<Claim>,
    /// Where it came from.
    pub source: Source,
    /// What an autoconfig document offers for OAuth2 sign-in; `None` from every other source and
    /// from a document that offers none. Data only: which provider an issuer names stays with the
    /// caller (mailo's `issuer_named` and `issuer_for_server`).
    pub oauth: Option<OAuthOffer>,
    /// POP3 servers an autoconfig document lists, never plain, implicit TLS first. Empty unless
    /// the search was asked to report them ([`crate::Pop3::Report`]); `endpoints` stay IMAP and
    /// SMTP whatever this holds.
    pub pop3: Vec<Pop3Server>,
}

/// Which half of an account a server serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `incomingServer`.
    Incoming,
    /// `outgoingServer`.
    Outgoing,
}

/// A server that lists `OAuth2` among its authentications, its host's placeholders filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthServer {
    /// Which half it serves.
    pub direction: Direction,
    /// The host.
    pub host: String,
    /// The port.
    pub port: u16,
}

/// The OAuth2 a document offers: the authorization server it names, if any, and the servers that
/// take OAuth2, in document order (incoming first). Present only when some server takes OAuth2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthOffer {
    /// The `oAuth2/issuer` text, as written.
    pub issuer: Option<String>,
    /// The servers that offer `OAuth2`.
    pub servers: Vec<OAuthServer>,
}

/// A POP3 server a document lists. Reported as data: porter's endpoints are IMAP and SMTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pop3Server {
    /// The host, placeholders filled in.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Implicit TLS or `STARTTLS`; a server with no TLS is never listed.
    pub tls: porter_core::Tls,
    /// The login, resolved against the address.
    pub login: porter_core::LoginName,
}

/// How the mail search treats a document that is usable only in part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// A document whose servers are `STARTTLS` only.
    pub starttls_only: StartTlsOnly,
    /// A document's POP3 servers.
    pub pop3: Pop3,
    /// A document whose servers take OAuth2 and no password.
    pub oauth_only: OAuthOnly,
}

/// What an OAuth2-only document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OAuthOnly {
    /// A miss: no password endpoint can be built (porter's default; providers that know the
    /// issuer take the address instead).
    #[default]
    Miss,
    /// A finding: a server that lists `OAuth2` serves as an endpoint, and [`Found::oauth`] says
    /// which. The caller must sign in with OAuth2 or drop the finding; this crate does not say
    /// the password rule was met.
    Offer,
}

/// What a `STARTTLS`-only document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartTlsOnly {
    /// Usable: the endpoint says `Tls::StartTls` (porter's default).
    #[default]
    Accept,
    /// A miss: only implicit-TLS servers count, and the search tries the next source (mailo's rule).
    TryNext,
}

/// What a document's POP3 servers are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pop3 {
    /// Not looked at: a POP3-only document is a miss (porter's default).
    #[default]
    Ignore,
    /// Listed on [`Found::pop3`]; a document with POP3 and SMTP but no IMAP is a finding.
    Report,
}

/// Why a step found nothing to offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DiscoverFault {
    /// The domain answered and named no usable server.
    #[error("no servers named")]
    NoServers,
    /// Nothing could be reached to ask.
    #[error("unreachable")]
    Unreachable,
    /// The answer was not the format the step reads.
    #[error("unreadable answer")]
    Unreadable,
}
