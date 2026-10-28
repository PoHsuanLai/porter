//! The connections of a mirror: every one is a relay accountd opens (`Tokens.OpenAuthenticated`)
//! to the account's CalDAV or CardDAV endpoint, so syncd never holds the credential. The same
//! dial serves discovery's PROPFINDs and the collection replicas' sync-collection reports.
//!
//! The collection's URL is not under the endpoint's path in general (the endpoint is
//! `.../remote.php/dav/`, a calendar is `.../calendars/<user>/personal/`), which
//! [`crate::webdav::webdav_replica`] cannot express (it takes a folder below the endpoint), so
//! the replica is built here over the collection's own URL; the relay only checks the origin.

use porter_client::{Grants, GrantsLink};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_http_relay::{RelayDial, RelayHttp, RelayLimits};
use porter_ids::WebUrl;
use porter_sync::Clock;
use std::sync::Arc;
use storage_webdav::WebDavReplica;

/// The HTTP seam over relays to `endpoint`.
pub fn pim_http<T: GrantsLink>(
    accounts: Arc<Grants<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
) -> RelayHttp<RelayDial<T>> {
    RelayHttp::new(
        RelayDial::authenticated(accounts, grant, endpoint),
        RelayLimits::default(),
    )
}

/// The replica of the collection at `collection`, over relays to `endpoint`.
pub fn pim_replica<T: GrantsLink>(
    accounts: Arc<Grants<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    collection: &WebUrl,
    clock: Clock,
    nonce: &str,
) -> WebDavReplica<RelayHttp<RelayDial<T>>> {
    WebDavReplica::new(
        pim_http(accounts, grant, endpoint),
        collection,
        clock,
        nonce,
    )
}
