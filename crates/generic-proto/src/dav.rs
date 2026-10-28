//! Finding a generic DAV server from the name a person typed: `.well-known/caldav` and
//! `.well-known/carddav` on a bare server, or the address itself when it names a path, and a
//! PROPFIND with the password at each to see that it is accepted (dav-proto's walk to the
//! principal).

use crate::{Error, bodiless, web};
use dav_proto::discover::{Error as WalkError, Reach, Scope, Walk};
use http::header::AUTHORIZATION;
use http::{HeaderValue, Method, Request, Response, Uri};
use porter_capability::{AbsentReason, CapabilityKind, Claim, Offer, Provenance, Subject};
use porter_core::{EndpointUrl, Family};
use porter_provider::ProviderSpec;

/// One DAV service a generic server may offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service {
    /// The capability it is.
    pub kind: CapabilityKind,
    /// The family of its endpoint.
    pub family: Family,
    /// Where a bare server publishes it (RFC 6764).
    pub well_known: &'static str,
}

/// The two DAV services a generic server may offer.
pub const SERVICES: [Service; 2] = [
    Service {
        kind: CapabilityKind::Calendar,
        family: Family::CalDav,
        well_known: "/.well-known/caldav",
    },
    Service {
        kind: CapabilityKind::Contacts,
        family: Family::CardDav,
        well_known: "/.well-known/carddav",
    },
];

/// Where to look for a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Root {
    /// The address names the service itself (it has a path), or names nowhere (`None`).
    Is(Option<EndpointUrl>),
    /// A bare server: ask this `.well-known` URL where the service is.
    WellKnown(EndpointUrl),
}

/// Where `service` is on `server`: the address itself when it names a path, else wherever its
/// `.well-known` points.
pub fn root(server: &EndpointUrl, service: &Service) -> Root {
    if server.path() != "/" {
        return Root::Is(server.join("/"));
    }
    match server.join(service.well_known) {
        Some(url) => Root::WellKnown(url),
        None => Root::Is(None),
    }
}

/// The request for a `.well-known` URL.
pub fn well_known_request(url: &EndpointUrl) -> Result<Request<Vec<u8>>, Error> {
    bodiless(Method::GET, url, None)
}

/// The PROPFIND that asks `url` for its principal with the account's `authorization`, and the
/// walk that reads the answer ([`accepted`]).
pub fn probe(
    url: &EndpointUrl,
    authorization: HeaderValue,
) -> Result<(Walk, Request<Vec<u8>>), Error> {
    let uri: Uri = web(url)?.as_str().parse().map_err(|_| Error::Unreadable)?;
    let (walk, mut request) = Walk::start(uri, Reach::Principal, &[], Scope::SameOrigin);
    request.headers_mut().insert(AUTHORIZATION, authorization);
    Ok((walk, request))
}

/// Whether the server answered the probe: `true` for a success (whatever it put in the body),
/// `false` when it does not speak DAV there, a refusal or a failure as the error.
pub fn accepted(walk: &mut Walk, response: &Response<Vec<u8>>) -> Result<bool, Error> {
    match walk.advance(response) {
        Ok(_) | Err(WalkError::Unreadable) => Ok(true),
        Err(WalkError::Unauthorized) => Err(Error::Refused),
        Err(WalkError::Status(500..=599)) => Err(Error::Unreachable),
        Err(_) => Ok(false),
    }
}

/// The spec's row for `kind` as found, or the kind missing on this server.
pub fn claim(spec: &ProviderSpec, kind: CapabilityKind, present: bool) -> Claim {
    let row = spec
        .capabilities
        .iter()
        .find(|row| row.capability.kind() == kind);
    let offer = match (present, row) {
        (true, Some(row)) => Offer::Present(row.capability.clone()),
        _ => Offer::Absent {
            kind,
            reason: AbsentReason::NotOnServer,
        },
    };
    Claim {
        subject: Subject::Account,
        offer,
        provenance: Provenance::Discovered,
    }
}

#[cfg(test)]
mod tests;
