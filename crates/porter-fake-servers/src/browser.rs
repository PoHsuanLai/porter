//! The scripted "browser": follows an authorize URL the way a person's browser would, and hits
//! the loopback redirect the issuer sends it to. It stands in for the one human step of an
//! OAuth sign-in.

use crate::http::{Request, Response, Scheme, send};
use porter_fake::FakeAddress;
use std::io;

/// What the browser saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    /// Where the issuer sent it (the `Location` of the authorize answer), if it was sent anywhere.
    pub redirected_to: Option<String>,
    /// What the page it landed on (the app's loopback server) answered, or the authorize answer
    /// itself when there was no redirect.
    pub landing: Response,
}

/// An `http://` loopback URL as the address and the request target.
pub fn split_loopback(url: &str) -> io::Result<(FakeAddress, String)> {
    let bad = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a loopback http URL: {url}"),
        )
    };
    let rest = url.strip_prefix("http://").ok_or_else(bad)?;
    let (authority, target) = rest
        .find('/')
        .map_or((rest, "/"), |at| (&rest[..at], &rest[at..]));
    let (host, port) = authority.split_once(':').ok_or_else(bad)?;
    match (host, port.parse::<u16>()) {
        ("127.0.0.1" | "localhost", Ok(port)) => {
            Ok((FakeAddress::Loopback(port), target.to_owned()))
        }
        _ => Err(bad()),
    }
}

/// Opens `authorize_url`; when the issuer answers with a redirect, opens that too (the app's
/// loopback redirect), and returns what both said.
pub async fn follow(authorize_url: &str) -> io::Result<Visit> {
    let (address, target) = split_loopback(authorize_url)?;
    let first = send(&address, Scheme::Http, &Request::new("GET", &target)).await?;
    let Some(location) = first.header("location").map(str::to_owned) else {
        return Ok(Visit {
            redirected_to: None,
            landing: first,
        });
    };
    let (back, back_target) = split_loopback(&location)?;
    let landing = send(&back, Scheme::Http, &Request::new("GET", &back_target)).await?;
    Ok(Visit {
        redirected_to: Some(location),
        landing,
    })
}
