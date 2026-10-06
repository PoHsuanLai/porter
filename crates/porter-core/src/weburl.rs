//! A page or an HTTP resource, with its query: what a browser is sent to and what an HTTP
//! request goes to. An [`EndpointUrl`] names a server and has no query; an OAuth authorize URL is
//! all query, so the two are different types.
//!
//! A `WebUrl` is `https://host[:port][/path][?query]`, or `http` to this computer only (a
//! loopback redirect, a local model server). No user info, no fragment; the text is ASCII with
//! nothing a header or a command line would choke on. Its serde form is the text.

use crate::endpoint::{EndpointUrl, Origin, UrlScheme, split_authority};
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The longest URL accepted; authorize URLs with a few scopes are well under it.
const MAX_LEN: usize = 4096;

/// A web URL; see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WebUrl(String);

impl WebUrl {
    /// The URL written as `text`, or why it is not a web URL.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let bad = || CoreError::MalformedId {
            what: "web url",
            text: text.to_owned(),
        };
        let usable = text.len() <= MAX_LEN
            && text.bytes().all(|b| b.is_ascii_graphic())
            && !text.contains('#');
        if !usable {
            return Err(bad());
        }
        let (origin, tail) = split(text).ok_or_else(bad)?;
        let scheme = origin.scheme;
        if scheme == UrlScheme::Http && !origin.is_loopback() {
            return Err(bad());
        }
        // A query with no path gets the root path, so the text is a valid request target.
        let normal = match tail.starts_with('?') {
            true => format!("{}/{tail}", &text[..text.len() - tail.len()]),
            false => text.to_owned(),
        };
        Ok(Self(normal))
    }

    /// The URL's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The scheme, host and port.
    pub fn origin(&self) -> Origin {
        // `parse` checked the split, and no other constructor builds one from text.
        split(&self.0)
            .map(|(origin, _)| origin)
            .unwrap_or_else(|| unreachable!("a WebUrl was split successfully when it was parsed"))
    }

    /// The path, `/` when the URL has none.
    pub fn path(&self) -> &str {
        let rest = self.0.split_once("://").map_or("", |(_, rest)| rest);
        let rest = rest.split('?').next().unwrap_or("");
        rest.find('/').map_or("/", |at| &rest[at..])
    }

    /// The query, without its `?`; `None` when there is none.
    pub fn query(&self) -> Option<&str> {
        self.0.split_once('?').map(|(_, query)| query)
    }
}

/// The origin of `https://authority[...]` or `http://authority[...]`, and what follows the
/// authority (a path, a query, or nothing). `None` for any other scheme, user info, or an
/// authority that is not `host[:port]`.
fn split(text: &str) -> Option<(Origin, &str)> {
    let (scheme, rest) = text.split_once("://")?;
    let scheme = match scheme {
        "https" => UrlScheme::Https,
        "http" => UrlScheme::Http,
        _ => return None,
    };
    let at = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(at);
    if authority.contains('@') {
        return None;
    }
    let (host, port) = split_authority(authority)?;
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|p| *p != 0)?,
        None => scheme.default_port(),
    };
    let host = host.to_ascii_lowercase();
    Some((Origin { scheme, host, port }, tail))
}

impl TryFrom<&EndpointUrl> for WebUrl {
    type Error = CoreError;
    /// An endpoint URL that is a web URL: `https`, or `http` to this computer.
    fn try_from(url: &EndpointUrl) -> Result<Self, CoreError> {
        Self::parse(url.as_str())
    }
}

impl TryFrom<String> for WebUrl {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<WebUrl> for String {
    fn from(url: WebUrl) -> String {
        url.0
    }
}

impl fmt::Display for WebUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
