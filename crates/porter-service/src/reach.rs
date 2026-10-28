//! Where an account's relays may go: the one policy the relay planning asks, and that the relay
//! itself then applies as a value in its [`RelayPlan`](porter_core::RelayPlan).
//!
//! - **Which host and port.** A relay dials the endpoint of the account that serves the grant's
//!   kind. Another origin is dialled only when the account's provider file declares it for a row
//!   of that kind: `auth_origins` (relayed with the bearer) or `linked_origins` (relayed with
//!   nothing added). Only a bare origin, and only on the scheme of the endpoint it belongs to.
//! - **Which paths.** One origin and one password often serve several kinds of an account (a
//!   Nextcloud app password reaches its files, calendars, contacts and the OCS API), so a relay
//!   planned for a grant of one kind stays under the path of that kind's endpoint: a contacts
//!   grant on Nextcloud reaches `/remote.php/dav/addressbooks/users/ada/` and not
//!   `/remote.php/dav/files/` or `/ocs/`.
//!
//! The reach of each family, from the endpoint's path:
//! - CalDAV, CardDAV, WebDAV, Nextcloud Notes: the endpoint's path. CalDAV and CardDAV also
//!   read the account's principal (where a client finds its calendar or address book home): a
//!   `PROPFIND` with `Depth: 0` under the `principals` collection beside the `calendars` or
//!   `addressbooks` tree the endpoint is in (`/remote.php/dav/principals/`).
//! - Every other family: the whole origin. Their bearer is per kind instead (Graph's and
//!   Google's tokens carry the grant's kind's scopes alone, so a calendar token on
//!   `www.googleapis.com` cannot read Drive), and their clients build paths from the origin
//!   (Drive's `/upload/drive/v3`, the Picker's `/v1/sessions`); a JMAP session names its API,
//!   upload and download URLs anywhere on the origin; a relay to a declared auth or linked origin
//!   has the origin's root as its endpoint.

use crate::registry::serves;
use porter_capability::CapabilityKind;
use porter_core::{Account, EndpointUrl, Family, PathReach, Reach, ReachMethods, ServiceEndpoint};
use porter_provider::ProviderSpec;
use porter_wire::Refusal;

/// The other origins a provider file may declare for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Declared {
    /// `auth_origins`: relayed with the bearer.
    Auth,
    /// `linked_origins`: relayed with nothing added.
    Linked,
}

/// The endpoint of `account` for a relay to `origin`, an origin that is not one of the account's
/// endpoints but that a row of `kind` declares: the home endpoint's own endpoint with the
/// origin's address, so the family and login are the row's and nothing else changes.
/// `EndpointNotGranted` for any origin the file does not declare, one with a path, or one on
/// another scheme than the endpoint it belongs to.
pub(crate) fn declared_endpoint(
    account: &Account,
    spec: &ProviderSpec,
    kind: CapabilityKind,
    origin: &EndpointUrl,
    declared: Declared,
) -> Result<ServiceEndpoint, Refusal> {
    let dialled = origin.origin();
    let bare = origin.path() == "/" && !origin.as_str().ends_with('/');
    let names = |endpoint: &ServiceEndpoint| {
        spec.capabilities
            .iter()
            .filter(|row| row.family == endpoint.family && row.capability.kind() == kind)
            .any(|row| match declared {
                Declared::Auth => row.auth_origins.iter().any(|o| o.allows(&dialled)),
                Declared::Linked => row.linked_origins.iter().any(|o| o.allows(&dialled)),
            })
    };
    let home = account
        .endpoints
        .iter()
        .filter(|e| serves(e, kind))
        .find(|e| bare && e.url.origin().scheme == dialled.scheme && names(e))
        .ok_or(Refusal::EndpointNotGranted)?;
    let endpoint = ServiceEndpoint {
        url: EndpointUrl::parse(&dialled.to_string()).map_err(|_| Refusal::EndpointNotGranted)?,
        ..home.clone()
    };
    endpoint.check().map_err(|_| Refusal::EndpointNotGranted)?;
    Ok(endpoint)
}

/// Whether `family`'s relays stay under their endpoint's path.
fn confined(family: Family) -> bool {
    matches!(
        family,
        Family::CalDav | Family::CardDav | Family::WebDav | Family::NextcloudNotes
    )
}

/// `%XX` escapes decoded to bytes; `None` for a malformed one (a `%` without two hex digits),
/// which a server may read as it likes.
fn decoded(text: &str) -> Option<Vec<u8>> {
    porter_ids::percent::is_well_formed(text).then(|| porter_ids::percent::decode_bytes(text))
}

fn trimmed(mut path: Vec<u8>) -> Vec<u8> {
    while path.last() == Some(&b'/') {
        path.pop();
    }
    path
}

/// The `principals` collection beside the `tree` the endpoint's path is in.
fn principals(prefix: &[u8], tree: &[u8]) -> Option<Vec<u8>> {
    let segments: Vec<&[u8]> = prefix.split(|b| *b == b'/').collect();
    let at = segments.iter().position(|s| *s == tree)?;
    let mut root = segments[..at].join(&b'/');
    root.extend_from_slice(b"/principals");
    Some(root)
}

/// Where a relay to `endpoint` may go.
pub(crate) fn reach_for(endpoint: &ServiceEndpoint) -> Reach {
    let family = endpoint.family;
    let prefix = decoded(endpoint.url.path())
        .map(trimmed)
        .unwrap_or_default();
    if !confined(family) || prefix.is_empty() {
        return Reach::origin();
    }
    let tree: Option<&[u8]> = match family {
        Family::CalDav => Some(b"calendars"),
        Family::CardDav => Some(b"addressbooks"),
        _ => None,
    };
    let principal = tree
        .and_then(|tree| principals(&prefix, tree))
        .map(|prefix| PathReach::new(prefix, ReachMethods::ReadProperties));
    let mut paths = vec![PathReach::new(prefix, ReachMethods::Any)];
    paths.extend(principal);
    Reach::under(paths)
}

#[cfg(test)]
mod tests;
