//! `.well-known` URLs for CalDAV, CardDAV and JMAP, and the JMAP session resource.

use crate::found::{DiscoverFault, Found, Source};
use crate::mail::mail_claim;
use porter_core::capability::{Access, Capability, CapabilityKind, Delta, MailTransport, Offered};
use porter_core::capability::{PimCap, PimTransport};
use porter_core::{
    Claim, EndpointUrl, Family, LoginName, Offer, Provenance, ServiceEndpoint, Subject, Tls,
};
use porter_http::{Http, HttpRequest, HttpResponse, Method};
use porter_provider::DomainName;
use serde::Deserialize;
use std::collections::BTreeMap;

const CORE: &str = "urn:ietf:params:jmap:core";
const MAIL: &str = "urn:ietf:params:jmap:mail";
const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
const CALENDARS: &str = "urn:ietf:params:jmap:calendars";

/// The `.well-known` path and the family that speaks it, for the kinds that have one.
fn well_known_of(kind: CapabilityKind) -> Option<(&'static str, Family)> {
    match kind {
        CapabilityKind::Calendar | CapabilityKind::Tasks => {
            Some(("/.well-known/caldav", Family::CalDav))
        }
        CapabilityKind::Contacts => Some(("/.well-known/carddav", Family::CardDav)),
        CapabilityKind::Mail => Some(("/.well-known/jmap", Family::Jmap)),
        _ => None,
    }
}

/// The `.well-known` URLs to try for `kind` at `domain`: `/.well-known/caldav` for calendars and
/// tasks, `/.well-known/carddav` for contacts, `/.well-known/jmap` for mail. Empty for a kind
/// with none. Always HTTPS.
pub fn well_known_urls(domain: &DomainName, kind: CapabilityKind) -> Vec<EndpointUrl> {
    well_known_of(kind)
        .and_then(|(path, _)| EndpointUrl::parse(&format!("https://{domain}{path}")).ok())
        .into_iter()
        .collect()
}

/// What a `.well-known` answer says about `kind` at the URL that was asked (`requested`).
///
/// A redirect names the service's root (resolved against the asked URL, `https` only: a
/// redirect to plain HTTP is `Unreadable`). A success, or `401` (a DAV server that wants a login
/// before it says anything), means the service is at the asked URL itself. `404` is `NoServers`.
pub fn well_known_found(
    kind: CapabilityKind,
    requested: &EndpointUrl,
    response: &HttpResponse,
    login: &LoginName,
) -> Result<Found, DiscoverFault> {
    let (_, family) = well_known_of(kind).ok_or(DiscoverFault::NoServers)?;
    let url = match response.status.0 {
        301 | 302 | 303 | 307 | 308 => redirect_target(requested, response.header("location"))?,
        200..=299 | 401 => requested.clone(),
        404 | 410 => return Err(DiscoverFault::NoServers),
        _ => return Err(DiscoverFault::Unreadable),
    };
    Ok(Found {
        endpoints: vec![ServiceEndpoint {
            family,
            url,
            tls: Tls::Implicit,
            login: login.clone(),
        }],
        claims: vec![well_known_claim(kind)],
        source: Source::WellKnown,
    })
}

/// Asks each `.well-known` URL for `kind` at `domain`, in order; the first that names a
/// service wins. The `Http` implementation does not follow redirects, so a `Location` is read
/// here.
pub async fn discover_well_known<H: Http>(
    http: &H,
    domain: &DomainName,
    kind: CapabilityKind,
    login: &LoginName,
) -> Result<Found, DiscoverFault> {
    let mut fault = DiscoverFault::NoServers;
    for url in well_known_urls(domain, kind) {
        match http.send(HttpRequest::new(Method::Get, url.clone())).await {
            Err(_) => fault = DiscoverFault::Unreachable,
            Ok(response) => match well_known_found(kind, &url, &response, login) {
                Ok(found) => return Ok(found),
                Err(why) => fault = why,
            },
        }
    }
    Err(fault)
}

/// The redirect's target: an absolute `https` URL, or an absolute path on the asked origin.
fn redirect_target(
    requested: &EndpointUrl,
    location: Option<&str>,
) -> Result<EndpointUrl, DiscoverFault> {
    let location = location.ok_or(DiscoverFault::Unreadable)?.trim();
    let origin = requested.origin();
    let port = match origin.port {
        443 => String::new(),
        other => format!(":{other}"),
    };
    let text = match location.starts_with('/') {
        true => format!("https://{}{port}{location}", origin.host),
        false => location.to_owned(),
    };
    let url = EndpointUrl::parse(&text).map_err(|_| DiscoverFault::Unreadable)?;
    match url.origin().scheme == porter_core::UrlScheme::Https {
        true => Ok(url),
        false => Err(DiscoverFault::Unreadable),
    }
}

fn well_known_claim(kind: CapabilityKind) -> Claim {
    let pim = |transport| PimCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        transport,
        collections: Offered::Present,
    };
    let offer = match kind {
        CapabilityKind::Calendar => Offer::Present(Capability::Calendar(pim(PimTransport::CalDav))),
        CapabilityKind::Tasks => Offer::Present(Capability::Tasks(pim(PimTransport::CalDav))),
        CapabilityKind::Contacts => {
            Offer::Present(Capability::Contacts(pim(PimTransport::CardDav)))
        }
        _ => return mail_claim(MailTransport::Jmap, Offered::Absent),
    };
    Claim {
        subject: Subject::Account,
        offer,
        provenance: Provenance::Discovered,
    }
}

/// The parts of a JMAP session resource (RFC 8620 section 2) that discovery reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    api_url: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    capabilities: BTreeMap<String, serde_json::Value>,
}

/// The endpoints and capabilities a JMAP session resource names: the `apiUrl` as the JMAP
/// endpoint, logged in as the session's `username`; mail (sending only when the submission
/// capability is there), contacts and calendars when their capabilities are.
pub fn parse_jmap_session(json: &str) -> Result<Found, DiscoverFault> {
    let session: Session = serde_json::from_str(json).map_err(|_| DiscoverFault::Unreadable)?;
    if !session.capabilities.contains_key(CORE) {
        return Err(DiscoverFault::Unreadable);
    }
    let url = EndpointUrl::parse(&session.api_url).map_err(|_| DiscoverFault::Unreadable)?;
    let tls = match url.origin().scheme == porter_core::UrlScheme::Https {
        true => Tls::Implicit,
        false => Tls::Plain,
    };
    let endpoint = ServiceEndpoint {
        family: Family::Jmap,
        url,
        tls,
        login: LoginName(session.username),
    };
    endpoint.check().map_err(|_| DiscoverFault::NoServers)?;
    let has = |name: &str| session.capabilities.contains_key(name);
    let send = match has(SUBMISSION) {
        true => Offered::Present,
        false => Offered::Absent,
    };
    let claims = [
        has(MAIL).then(|| mail_claim(MailTransport::Jmap, send)),
        has(CONTACTS).then(|| jmap_pim(Capability::Contacts)),
        has(CALENDARS).then(|| jmap_pim(Capability::Calendar)),
    ]
    .into_iter()
    .flatten()
    .collect();
    Ok(Found {
        endpoints: vec![endpoint],
        claims,
        source: Source::JmapSession,
    })
}

fn jmap_pim(wrap: fn(PimCap) -> Capability) -> Claim {
    Claim {
        subject: Subject::Account,
        offer: Offer::Present(wrap(PimCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            transport: PimTransport::Jmap,
            collections: Offered::Present,
        })),
        provenance: Provenance::Discovered,
    }
}

#[cfg(test)]
mod tests;
