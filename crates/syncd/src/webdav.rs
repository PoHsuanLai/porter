//! The WebDAV replica as syncd runs it: connections come from accountd's
//! `Tokens.OpenAuthenticated`, so syncd never holds the account's credential (porter PLAN G3, D8).
//!
//! [`RelayDial`] asks accountd for one authenticated stream to the account's WebDAV endpoint each
//! time the replica needs a connection; the relay at the far end adds `Authorization`, speaks TLS
//! and reaches that endpoint's origin only. [`webdav_replica`] builds the replica of a dataset's
//! folder under that endpoint over them.

use porter_client::{Grants, GrantsLink};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_http_relay::{RelayDial, RelayHttp, RelayLimits};
use porter_ids::WebUrl;
use porter_sync::Clock;
use std::sync::Arc;
use storage_webdav::WebDavReplica;

/// Why a replica could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// The endpoint is not a web URL, or the folder makes it not one.
    #[error("the endpoint and folder do not make a web URL")]
    NotWeb,
}

/// The replica of `folder` (a path under the endpoint, `Photos/Originals`) of the account the
/// grant covers, over relays accountd opens to `endpoint`.
pub fn webdav_replica<T: GrantsLink>(
    accounts: Arc<Grants<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    folder: &str,
    clock: Clock,
    nonce: &str,
) -> Result<WebDavReplica<RelayHttp<RelayDial<T>>>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let url = WebUrl::parse(&format!(
        "{}/{}/",
        base.as_str().trim_end_matches('/'),
        folder.trim_matches('/')
    ))
    .map_err(|_| BuildError::NotWeb)?;
    let dial = RelayDial::authenticated(accounts, grant, endpoint);
    Ok(WebDavReplica::new(
        RelayHttp::new(dial, RelayLimits::default()),
        &url,
        clock,
        nonce,
    ))
}
