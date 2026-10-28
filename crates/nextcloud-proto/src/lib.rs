//! Nextcloud's own protocol, with no IO: requests are `http::Request<Vec<u8>>` values and replies
//! are read from `&http::Response<Vec<u8>>`, so the caller sends them with whatever HTTP client
//! it has and recorded bodies drive every test.
//!
//! - [`flow`]: Login Flow v2, which a browser approves and which mints an app password.
//! - [`ocs`]: the capabilities request and the answer read as claims.
//! - [`revoke`]: taking the app password back.
//! - [`paths`]: where a server keeps things, and the account's endpoints ([`endpoints`]).
//!
//! The WebDAV walk (the principal, the home sets) is dav-proto's. The credential is the caller's:
//! each request takes the `Authorization` value it carries, already marked sensitive.

#![warn(missing_docs)]

pub mod flow;
pub mod ocs;
pub mod paths;
pub mod revoke;

use http::header::AUTHORIZATION;
use http::{HeaderValue, Request, Response};
use porter_capability::{CapabilityKind, Claim, offers};
use porter_core::{EndpointUrl, Family, LoginName, ServiceEndpoint};
use porter_ids::{UrlScheme, WebUrl};

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
    /// A reply, or an address, that is not what Nextcloud sends.
    #[error("not a Nextcloud answer")]
    Unreadable,
}

/// `url` as a web URL: `https`, or `http` to this computer.
pub(crate) fn web(url: &EndpointUrl) -> Result<WebUrl, Error> {
    WebUrl::try_from(url).map_err(|_| match url.origin().scheme {
        UrlScheme::Http => Error::Insecure,
        _ => Error::Unreadable,
    })
}

/// `request` with `authorization` as its `Authorization` header.
pub(crate) fn authorized(
    mut request: Request<Vec<u8>>,
    authorization: HeaderValue,
) -> Request<Vec<u8>> {
    request.headers_mut().insert(AUTHORIZATION, authorization);
    request
}

/// A reply that is a login refusal (401, 403), a server failure (5xx), another failure, or a
/// success (2xx), which is returned.
pub fn judged(response: &Response<Vec<u8>>) -> Result<&Response<Vec<u8>>, Error> {
    match response.status().as_u16() {
        401 | 403 => Err(Error::Refused),
        200..=299 => Ok(response),
        500..=599 => Err(Error::Unreachable),
        _ => Err(Error::Unreadable),
    }
}

/// What the account's endpoints are, from what was found: the files root always; the calendar
/// and task home when the server offers either, the address book home for contacts, the Notes
/// API for notes. `calendars` and `books` are the homes the DAV walk named, else the
/// conventional paths. `None` when an endpoint cannot be made.
pub fn endpoints(
    server: &EndpointUrl,
    user: &str,
    login: &LoginName,
    claims: &[Claim],
    calendars: Option<EndpointUrl>,
    books: Option<EndpointUrl>,
) -> Option<Vec<ServiceEndpoint>> {
    let calendars = calendars.or_else(|| paths::calendars(server, user));
    let books = books.or_else(|| paths::address_books(server, user));
    let rows = [
        (Family::WebDav, paths::files(server, user), true),
        (
            Family::CalDav,
            calendars,
            offers(claims, CapabilityKind::Calendar) || offers(claims, CapabilityKind::Tasks),
        ),
        (
            Family::CardDav,
            books,
            offers(claims, CapabilityKind::Contacts),
        ),
        (
            Family::NextcloudNotes,
            paths::notes(server),
            offers(claims, CapabilityKind::Notes),
        ),
    ];
    rows.into_iter()
        .filter(|(_, _, wanted)| *wanted)
        .map(|(family, url, _)| url.and_then(|u| ServiceEndpoint::coherent(family, u, login)))
        .collect()
}

#[cfg(test)]
mod tests;
