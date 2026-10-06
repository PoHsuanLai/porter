//! Finding a generic DAV server from the name a person typed: `.well-known/caldav` and
//! `.well-known/carddav` on a bare server, or the address itself when it names a path, and a
//! PROPFIND with the password at each to see that it is accepted.

use crate::io::Io;
use crate::password::{basic, endpoint, fault_of, join};
use porter_core::capability::CapabilityKind;
use porter_core::sheet::SignInFault;
use porter_core::{
    AbsentReason, Claim, EndpointUrl, Family, LoginName, Offer, Provenance, SecretText,
    ServiceEndpoint, Subject,
};
use porter_dav::names::CURRENT_USER_PRINCIPAL;
use porter_dav::{Depth, propfind};
use porter_discover::well_known_found;
use porter_http::{Header, Http, HttpRequest, Method, web_url};
use porter_provider::ProviderSpec;

/// The two DAV services a generic server may offer, and the family of each.
const SERVICES: [(CapabilityKind, Family, &str); 2] = [
    (
        CapabilityKind::Calendar,
        Family::CalDav,
        "/.well-known/caldav",
    ),
    (
        CapabilityKind::Contacts,
        Family::CardDav,
        "/.well-known/carddav",
    ),
];

/// What was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Found {
    pub(super) claims: Vec<Claim>,
    pub(super) endpoints: Vec<ServiceEndpoint>,
}

/// Finds the services at `server` and checks `login`'s password at each.
pub(super) async fn discover(
    io: &Io,
    spec: &ProviderSpec,
    server: &EndpointUrl,
    login: &LoginName,
    password: &SecretText,
) -> Result<Found, SignInFault> {
    let mut verified = Vec::new();
    let mut claims = Vec::new();
    let mut unanswered = None;
    for (kind, family, well_known) in SERVICES {
        let present = match root(io, server, well_known, kind, login).await {
            Ok(Some(url)) if check(io, &url, login, password).await? => {
                verified.extend(endpoint(family, url, login));
                true
            }
            Ok(_) => false,
            Err(fault) => {
                unanswered = Some(fault);
                false
            }
        };
        claims.push(claim(spec, kind, present));
    }
    match (verified.is_empty(), unanswered) {
        (true, Some(fault)) => Err(fault),
        (true, None) => Err(SignInFault::Unreadable),
        (false, _) => Ok(Found {
            claims,
            endpoints: verified,
        }),
    }
}

/// Where `kind` is on `server`: the address itself when it names a path, else wherever
/// `.well-known` points.
async fn root(
    io: &Io,
    server: &EndpointUrl,
    well_known: &str,
    kind: CapabilityKind,
    login: &LoginName,
) -> Result<Option<EndpointUrl>, SignInFault> {
    if server.path() != "/" {
        return Ok(join(server, "/"));
    }
    let Some(requested) = join(server, well_known) else {
        return Ok(None);
    };
    let response = io
        .http
        .send(HttpRequest::to(Method::Get, &requested).map_err(fault_of)?)
        .await
        .map_err(fault_of)?;
    Ok(well_known_found(kind, &requested, &response, login)
        .ok()
        .and_then(|found| found.endpoints.into_iter().next())
        .map(|e| e.url))
}

/// Whether the server answers a PROPFIND at `url` with `login`'s password: `true`, `false` when
/// it does not speak DAV there, and a refusal as the fault.
async fn check(
    io: &Io,
    url: &EndpointUrl,
    login: &LoginName,
    password: &SecretText,
) -> Result<bool, SignInFault> {
    let mut request = propfind(
        web_url(url).map_err(fault_of)?,
        Depth::Zero,
        &[CURRENT_USER_PRINCIPAL],
    );
    request
        .headers
        .push(Header::new("Authorization", basic(&login.0, password)));
    let response = io.http.send(request).await.map_err(fault_of)?;
    match response.status.0 {
        401 | 403 => Err(SignInFault::Refused),
        200..=299 => Ok(true),
        500..=599 => Err(SignInFault::Unreachable),
        _ => Ok(false),
    }
}

/// The spec's row for `kind` as found, or the kind missing on this server.
fn claim(spec: &ProviderSpec, kind: CapabilityKind, present: bool) -> Claim {
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
