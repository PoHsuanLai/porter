//! `.well-known` URLs for CalDAV, CardDAV and JMAP, and the JMAP session resource.

use crate::found::{DiscoverFault, Found};
use porter_core::{CapabilityKind, EndpointUrl};
use porter_provider::DomainName;

/// The `.well-known` URLs to try for `kind` at `domain`.
pub fn well_known_urls(domain: &DomainName, kind: CapabilityKind) -> Vec<EndpointUrl> {
    let _ = (domain, kind);
    todo!("`/.well-known/caldav`, `/.well-known/carddav`, `/.well-known/jmap`")
}

/// The endpoints and capabilities a JMAP session resource names.
pub fn parse_jmap_session(json: &str) -> Result<Found, DiscoverFault> {
    let _ = json;
    todo!("read `apiUrl`, `accounts` and `capabilities` of the session")
}
