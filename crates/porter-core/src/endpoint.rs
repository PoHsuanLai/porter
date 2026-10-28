//! Where an account's servers are (design/31 §3.1, §4.4): which host and port speak which
//! protocol, with which TLS and login name. None of it is secret: it is stored per account,
//! returned with every candidate, and is what an app dials, or names to `OpenAuthenticated`.
//!
//! An endpoint is parsed once where it enters (a provider file, discovery, a legacy record, the
//! bus); everything past that holds typed values.

use crate::error::CoreError;
use crate::family::Family;
use crate::protocol::EndpointProtocol;
use crate::secret::SecretText;
use porter_capability::CapabilityKind;
use porter_ids::{IdError, Origin, UrlScheme, WebUrl, split_authority};
use serde::{Deserialize, Serialize};
use std::fmt;

mod reach;
mod server_address;

pub use reach::{PathReach, Reach, ReachMethods};
pub use server_address::{ServerAddress, ServerAddressFault, server_address, server_address_shape};

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

/// An endpoint's URL: `scheme://host[:port][/path]`, with no user info, query or fragment.
/// Its serde form is the text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EndpointUrl(String);

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

impl TryFrom<&EndpointUrl> for WebUrl {
    type Error = IdError;
    /// An endpoint URL that is a web URL: `https`, or `http` to this computer.
    fn try_from(url: &EndpointUrl) -> Result<Self, IdError> {
        Self::parse(url.as_str())
    }
}

/// The security a scheme allows: `https`, `imaps` and `smtps` are TLS from the first byte,
/// `http` has no upgrade, and `imap`, `smtp`, `sieve` and `pop3` upgrade with STARTTLS or
/// stay plain.
fn admits(scheme: UrlScheme, tls: Tls) -> bool {
    match scheme {
        UrlScheme::Https
        | UrlScheme::Imaps
        | UrlScheme::Smtps
        | UrlScheme::Sieves
        | UrlScheme::Pop3s => tls == Tls::Implicit,
        UrlScheme::Http => tls == Tls::Plain,
        UrlScheme::Imap | UrlScheme::Smtp | UrlScheme::Sieve | UrlScheme::Pop3 => {
            matches!(tls, Tls::StartTls | Tls::Plain)
        }
    }
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

/// Why an endpoint is not one porter will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
    ///
    /// One family is the exception: the address of a person's own AI server (`ChatCompletions`)
    /// may be plain `http` to one of their own computers on their tailnet, whose traffic the
    /// tailnet already encrypts. Whose computer it is only [`server_address`] (given the list of
    /// computers when the address was typed) can tell; this check must not refuse an account
    /// later because a computer has left the list, so it admits plain `http` for that family to
    /// an address only a tailnet hands out ([`Origin::is_tailnet`]), and to no other host. A
    /// relay never carries the family.
    pub fn check(&self) -> Result<(), EndpointFault> {
        let origin = self.url.origin();
        if self
            .protocol()
            .is_some_and(|p| p != EndpointProtocol::of_scheme(origin.scheme))
        {
            return Err(EndpointFault::SchemeMismatch);
        }
        if !admits(origin.scheme, self.tls) {
            return Err(EndpointFault::TlsMismatch);
        }
        let own_server = self.family == Family::ChatCompletions && origin.is_tailnet();
        match self.tls == Tls::Plain && !origin.is_loopback() && !own_server {
            true => Err(EndpointFault::PlainOffLoopback),
            false => Ok(()),
        }
    }
}

/// What a relay presents to its endpoint. Secret text: its `Debug` redacts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelayAuth {
    /// The account's password or app password: IMAP `LOGIN`, SMTP `AUTH PLAIN`, HTTP Basic.
    Password(SecretText),
    /// A short-lived access token, minted by the provider session: `XOAUTH2` for IMAP and
    /// SMTP, a bearer header for HTTP.
    AccessToken(SecretText),
    /// Nothing: the stream reaches a linked origin (`OpenLinked`) whose URLs carry their own
    /// authorisation, and a credential sent there would be refused or leaked. Only the HTTP
    /// relay carries it; the password protocols refuse it.
    Anonymous,
}

/// What a relay is told: the one endpoint it dials and what it presents there. It stays inside
/// the host that runs the relay; it is never serialised.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RelayPlan {
    /// The only server the relay talks to.
    pub endpoint: ServiceEndpoint,
    /// The kind of the grant it serves (the audit line names it).
    pub kind: CapabilityKind,
    /// What it presents.
    pub auth: RelayAuth,
    /// The paths of the endpoint's origin it may send requests to.
    pub reach: Reach,
}

impl RelayPlan {
    /// A plan to dial `endpoint` for a grant of `kind`, presenting `auth` there and going no
    /// further than `reach`.
    pub fn new(
        endpoint: ServiceEndpoint,
        kind: CapabilityKind,
        auth: RelayAuth,
        reach: Reach,
    ) -> Self {
        Self {
            endpoint,
            kind,
            auth,
            reach,
        }
    }
}

#[cfg(test)]
mod tests;
