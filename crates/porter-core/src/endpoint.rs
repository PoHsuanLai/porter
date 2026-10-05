//! Where an account's servers are (design/31 §3.1, §4.4): which host and port speak which
//! protocol, with which TLS and login name. None of it is secret: it is stored per account,
//! returned with every candidate, and is what an app dials, or names to `OpenAuthenticated`.
//!
//! An endpoint is parsed once where it enters (a provider file, discovery, a legacy record, the
//! bus); everything past that holds typed values.

use crate::capability::CapabilityKind;
use crate::error::CoreError;
use crate::family::Family;
use crate::secret::SecretText;
use serde::{Deserialize, Serialize};
use std::fmt;

/// One server of an account, as an app or the relay needs to reach it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    /// The protocol engine that speaks to it (`Imap` for the incoming mail server, `Smtp` for
    /// the outgoing one, `WebDav`, `CalDav`, `CardDav` for the root of each).
    pub family: Family,
    /// Where it is.
    pub url: EndpointUrl,
    /// How the connection is secured.
    pub tls: Tls,
    /// The name the server wants as the login (an address, a user name); not a secret.
    pub login: LoginName,
}

/// How a connection to an endpoint is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tls {
    /// TLS from the first byte (`imaps`, `smtps`, `https`).
    Implicit,
    /// Plain first, upgraded with STARTTLS before any credential is sent.
    StartTls,
    /// No TLS. Only a loopback host may be plain (`ServiceEndpoint::check`).
    Plain,
}

/// The login name an endpoint wants. Not a secret, so it has a plain `Debug`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LoginName(pub String);

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
}

/// The schemes an endpoint URL may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UrlScheme {
    /// `http`.
    Http,
    /// `https`.
    Https,
    /// `imap`.
    Imap,
    /// `imaps`.
    Imaps,
    /// `smtp`.
    Smtp,
    /// `smtps`.
    Smtps,
    /// `sieve` (ManageSieve, STARTTLS).
    Sieve,
    /// `sieves` (ManageSieve over TLS from the first byte).
    Sieves,
}

impl UrlScheme {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "http" => Some(UrlScheme::Http),
            "https" => Some(UrlScheme::Https),
            "imap" => Some(UrlScheme::Imap),
            "imaps" => Some(UrlScheme::Imaps),
            "smtp" => Some(UrlScheme::Smtp),
            "smtps" => Some(UrlScheme::Smtps),
            "sieve" => Some(UrlScheme::Sieve),
            "sieves" => Some(UrlScheme::Sieves),
            _ => None,
        }
    }

    fn text(self) -> &'static str {
        match self {
            UrlScheme::Http => "http",
            UrlScheme::Https => "https",
            UrlScheme::Imap => "imap",
            UrlScheme::Imaps => "imaps",
            UrlScheme::Smtp => "smtp",
            UrlScheme::Smtps => "smtps",
            UrlScheme::Sieve => "sieve",
            UrlScheme::Sieves => "sieves",
        }
    }

    /// The protocol this scheme names.
    pub fn protocol(self) -> EndpointProtocol {
        match self {
            UrlScheme::Http | UrlScheme::Https => EndpointProtocol::Http,
            UrlScheme::Imap | UrlScheme::Imaps => EndpointProtocol::Imap,
            UrlScheme::Smtp | UrlScheme::Smtps => EndpointProtocol::Smtp,
            UrlScheme::Sieve | UrlScheme::Sieves => EndpointProtocol::Sieve,
        }
    }

    /// The port used when the URL names none.
    pub fn default_port(self) -> u16 {
        match self {
            UrlScheme::Http => 80,
            UrlScheme::Https => 443,
            UrlScheme::Imap => 143,
            UrlScheme::Imaps => 993,
            UrlScheme::Smtp => 587,
            UrlScheme::Smtps => 465,
            UrlScheme::Sieve | UrlScheme::Sieves => 4190,
        }
    }

    /// The security a scheme allows: `https`, `imaps` and `smtps` are TLS from the first byte,
    /// `http` has no upgrade, and `imap`, `smtp` and `sieve` upgrade with STARTTLS or stay plain.
    fn admits(self, tls: Tls) -> bool {
        match self {
            UrlScheme::Https | UrlScheme::Imaps | UrlScheme::Smtps | UrlScheme::Sieves => {
                tls == Tls::Implicit
            }
            UrlScheme::Http => tls == Tls::Plain,
            UrlScheme::Imap | UrlScheme::Smtp | UrlScheme::Sieve => {
                matches!(tls, Tls::StartTls | Tls::Plain)
            }
        }
    }
}

/// An endpoint's URL: `scheme://host[:port][/path]`, with no user info, query or fragment.
/// Its serde form is the text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EndpointUrl(String);

/// Scheme, host and port: what "the same server" means.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    /// The scheme.
    pub scheme: UrlScheme,
    /// The host in lower case, an IPv6 literal without its brackets.
    pub host: String,
    /// The port, the scheme's default when the URL named none.
    pub port: u16,
}

impl EndpointUrl {
    /// The URL written as `text`, or why it is not an endpoint URL.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let bad = || CoreError::MalformedId {
            what: "endpoint url",
            text: text.to_owned(),
        };
        let usable = !text.is_empty()
            && text.len() <= 2048
            && text.bytes().all(|b| b.is_ascii_graphic())
            && !text.contains(['#', '?', '@']);
        if !usable {
            return Err(bad());
        }
        let candidate = Self(text.to_owned());
        match candidate.split().is_some() {
            true => Ok(candidate),
            false => Err(bad()),
        }
    }

    /// The URL's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The scheme, host and port.
    pub fn origin(&self) -> Origin {
        // `parse` checked the split, and no other constructor exists.
        let (scheme, host, port, _) = self.split().unwrap_or_else(|| {
            unreachable!("an EndpointUrl was split successfully when it was parsed")
        });
        Origin { scheme, host, port }
    }

    /// The path after the authority, `/` when there is none.
    pub fn path(&self) -> &str {
        self.split().map_or("/", |(_, _, _, path)| path)
    }

    fn split(&self) -> Option<(UrlScheme, String, u16, &str)> {
        let (scheme, rest) = self.0.split_once("://")?;
        let scheme = UrlScheme::parse(scheme)?;
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        let (host, port) = split_authority(authority)?;
        let port = match port {
            Some(text) => text.parse::<u16>().ok().filter(|p| *p != 0)?,
            None => scheme.default_port(),
        };
        Some((scheme, host.to_ascii_lowercase(), port, path))
    }
}

/// `host[:port]` or `[v6][:port]`; the host is never empty.
fn split_authority(authority: &str) -> Option<(&str, Option<&str>)> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'));
    host_ok.then_some((host, port))
}

impl TryFrom<String> for EndpointUrl {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<EndpointUrl> for String {
    fn from(url: EndpointUrl) -> String {
        url.0
    }
}

impl fmt::Display for EndpointUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Origin {
    /// Whether the host is this computer (`localhost`, `127.0.0.0/8`, `::1`).
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost" || self.host == "::1" || self.host.starts_with("127.")
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}:{}", self.scheme.text(), self.host, self.port)
    }
}

/// Why an endpoint is not one porter will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EndpointFault {
    /// The family does not speak the URL's scheme, or has no relay protocol.
    #[error("the family does not speak that scheme")]
    SchemeMismatch,
    /// The TLS mode does not go with the scheme.
    #[error("the TLS mode does not go with the scheme")]
    TlsMismatch,
    /// No TLS to a host that is not this computer.
    #[error("plain connections are for this computer only")]
    PlainOffLoopback,
}

impl ServiceEndpoint {
    /// The protocol a relay speaks to it, if the relay carries its family.
    pub fn protocol(&self) -> Option<EndpointProtocol> {
        self.family.relay_protocol()
    }

    /// Whether the endpoint is coherent: the scheme is the family's protocol, the TLS mode goes
    /// with the scheme, and only a loopback host is plain. Run where an endpoint enters (a file,
    /// discovery, the bus), so nothing past that has to ask again.
    pub fn check(&self) -> Result<(), EndpointFault> {
        let origin = self.url.origin();
        if self
            .protocol()
            .is_some_and(|p| p != origin.scheme.protocol())
        {
            return Err(EndpointFault::SchemeMismatch);
        }
        if !origin.scheme.admits(self.tls) {
            return Err(EndpointFault::TlsMismatch);
        }
        match self.tls == Tls::Plain && !origin.is_loopback() {
            true => Err(EndpointFault::PlainOffLoopback),
            false => Ok(()),
        }
    }
}

/// What a relay presents to its endpoint. Secret text: its `Debug` redacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayAuth {
    /// The account's password or app password: IMAP `LOGIN`, SMTP `AUTH PLAIN`, HTTP Basic.
    Password(SecretText),
    /// A short-lived access token, minted by the provider session: `XOAUTH2` for IMAP and
    /// SMTP, a bearer header for HTTP.
    AccessToken(SecretText),
}

/// What a relay is told: the one endpoint it dials and what it presents there. It stays inside
/// the host that runs the relay; it is never serialised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPlan {
    /// The only server the relay talks to.
    pub endpoint: ServiceEndpoint,
    /// The kind of the grant it serves (the audit line names it).
    pub kind: CapabilityKind,
    /// What it presents.
    pub auth: RelayAuth,
}

#[cfg(test)]
mod tests;
