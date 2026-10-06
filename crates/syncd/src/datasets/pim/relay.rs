//! The connections of a mirror: every one is a relay accountd opens (`Tokens.OpenAuthenticated`)
//! to the account's CalDAV or CardDAV endpoint, so syncd never holds the credential. The same
//! dial serves discovery's PROPFINDs and the collection replicas' sync-collection reports.
//!
//! The collection's URL is not under the endpoint's path in general (the endpoint is
//! `.../remote.php/dav/`, a calendar is `.../calendars/<user>/personal/`), which
//! [`crate::webdav::webdav_replica`] cannot express (it takes a folder below the endpoint), so
//! the replica is built here over the collection's own URL; the relay only checks the origin.

use crate::webdav::RelayStream;
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, WebUrl};
use std::sync::Arc;
use storage_webdav::{Clock, Dial, StreamHttp, StreamLimits, WebDavReplica};

/// Opens relays to one endpoint of one grant.
#[derive(Debug)]
pub struct PimDial<T> {
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
}

impl<T> PimDial<T> {
    /// A dial for `endpoint`, as `grant` allows.
    pub fn new(accounts: Arc<Accounts<T>>, grant: GrantId, endpoint: EndpointUrl) -> Self {
        Self {
            accounts,
            grant,
            endpoint,
        }
    }
}

impl<T: Transport> Dial for PimDial<T> {
    type Stream = RelayStream;

    async fn dial(&self) -> Result<RelayStream, porter_http::HttpError> {
        let stream = self
            .accounts
            .open_authenticated(&self.grant, &self.endpoint)
            .await
            .map_err(|_| porter_http::HttpError::Unreachable)?;
        RelayStream::from_relay(stream).map_err(|_| porter_http::HttpError::Unreachable)
    }
}

/// The HTTP seam over relays to `endpoint`.
pub fn pim_http<T: Transport>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
) -> StreamHttp<PimDial<T>> {
    StreamHttp::new(
        PimDial::new(accounts, grant, endpoint),
        StreamLimits::default(),
    )
}

/// The replica of the collection at `collection`, over relays to `endpoint`.
pub fn pim_replica<T: Transport>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    collection: &WebUrl,
    clock: Clock,
) -> WebDavReplica<StreamHttp<PimDial<T>>> {
    WebDavReplica::new(pim_http(accounts, grant, endpoint), collection, clock)
}
