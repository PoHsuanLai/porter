//! A generic account's servers, with no IO: requests are `http::Request<Vec<u8>>` values and
//! replies are read from `&http::Response<Vec<u8>>`, so the caller sends them with whatever HTTP
//! client it has and tests need no network.
//!
//! - [`mail`]: the mail servers a person typed, or a provider file names, as endpoints and claims.
//! - [`dav`]: finding CalDAV and CardDAV at a server: `.well-known`, then a PROPFIND with the
//!   password (dav-proto's walk).
//! - [`jmap`]: the session request of a JMAP server typed by hand.
//! - [`check`]: what the password is tried against at the incoming mail server.
//!
//! The credential is the caller's: each request takes the `Authorization` value it carries,
//! already marked sensitive.

#![warn(missing_docs)]

pub mod check;
pub mod dav;
pub mod jmap;
pub mod mail;

use http::header::AUTHORIZATION;
use http::{HeaderValue, Request};
use porter_capability::{Claim, Offer, Provenance, Subject};
use porter_core::EndpointUrl;
use porter_ids::{UrlScheme, WebUrl};
use porter_provider::ProviderSpec;

/// Why a request could not be built or a reply could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The server refused the credential (401 or 403).
    #[error("the server refused the account")]
    Refused,
    /// The server failed (5xx).
    #[error("the server failed")]
    Unreachable,
    /// The URL is plain `http` to another computer: no credential is sent in the clear.
    #[error("plain http to another computer")]
    Insecure,
    /// A reply, an address or a form that is not what the protocol says.
    #[error("not what the protocol says")]
    Unreadable,
}

/// `url` as a web URL: `https`, or `http` to this computer.
pub(crate) fn web(url: &EndpointUrl) -> Result<WebUrl, Error> {
    WebUrl::try_from(url).map_err(|_| match url.origin().scheme {
        UrlScheme::Http => Error::Insecure,
        _ => Error::Unreadable,
    })
}

/// A `GET` or other bodiless request to `url`, with `authorization` when given.
pub(crate) fn bodiless(
    method: http::Method,
    url: &EndpointUrl,
    authorization: Option<HeaderValue>,
) -> Result<Request<Vec<u8>>, Error> {
    let url = web(url)?;
    let mut request = Request::builder()
        .method(method)
        .uri(url.as_str())
        .body(Vec::new())
        .map_err(|_| Error::Unreadable)?;
    if let Some(authorization) = authorization {
        request.headers_mut().insert(AUTHORIZATION, authorization);
    }
    Ok(request)
}

/// What the provider file declares, as claims at `Declared` provenance.
pub fn declared(spec: &ProviderSpec) -> Vec<Claim> {
    spec.capabilities
        .iter()
        .map(|row| Claim {
            subject: Subject::Account,
            offer: Offer::Present(row.capability.clone()),
            provenance: Provenance::Declared,
        })
        .collect()
}
