//! Finding a generic DAV server from the name a person typed, sent: `.well-known/caldav` and
//! `.well-known/carddav` on a bare server, or the address itself when it names a path, and a
//! PROPFIND with the password at each to see that it is accepted. The requests and the verdicts
//! are generic-proto's.

use super::fault;
use generic_proto::dav::{self, Root, SERVICES, Service};
use porter_capability::Claim;
use porter_core::{EndpointUrl, LoginName, SecretText, ServiceEndpoint};
use porter_discover::well_known_found;
use porter_family_kit::Io;
use porter_family_kit::password::{basic, fault_of, sensitive};
use porter_http::Http;
use porter_provider::ProviderSpec;
use porter_sheet::SignInFault;

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
    for service in SERVICES {
        let present = match root(io, server, &service, login).await {
            Ok(Some(url)) if check(io, &url, login, password).await? => {
                verified.extend(ServiceEndpoint::coherent(service.family, url, login));
                true
            }
            Ok(_) => false,
            Err(fault) => {
                unanswered = Some(fault);
                false
            }
        };
        claims.push(dav::claim(spec, service.kind, present));
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

/// Where `service` is on `server`: the address itself when it names a path, else wherever
/// `.well-known` points.
async fn root(
    io: &Io,
    server: &EndpointUrl,
    service: &Service,
    login: &LoginName,
) -> Result<Option<EndpointUrl>, SignInFault> {
    let requested = match dav::root(server, service) {
        Root::Is(url) => return Ok(url),
        Root::WellKnown(requested) => requested,
    };
    let request = dav::well_known_request(&requested).map_err(fault)?;
    let response = io.http.send(request).await.map_err(fault_of)?;
    Ok(well_known_found(service.kind, &requested, &response, login)
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
    let authorization = sensitive(&basic(&login.0, password)).map_err(fault_of)?;
    let (mut walk, request) = dav::probe(url, authorization).map_err(fault)?;
    let response = io.http.send(request).await.map_err(fault_of)?;
    dav::accepted(&mut walk, &response).map_err(fault)
}
