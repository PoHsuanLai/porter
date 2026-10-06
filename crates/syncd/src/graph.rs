//! The Graph replica as syncd runs it: the same relays as the WebDAV replica
//! ([`crate::webdav::RelayDial`]: one authenticated stream per connection from accountd's
//! `Tokens.OpenAuthenticated`), so syncd never holds the Microsoft token. The relay adds
//! `Authorization: Bearer` and reaches the endpoint's origin (`https://graph.microsoft.com`).
//!
//! [`graph_replica`] builds the replica of a dataset's folder in the account's OneDrive app
//! folder. Which grant and endpoint a Storage account on a Microsoft account is reached through
//! is accountd's candidate: the `graph` endpoint under a Storage grant.

use crate::webdav::{BuildError, RelayDial};
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, WebUrl};
use std::sync::Arc;
use storage_graph::{Clock, GraphReplica, StreamHttp, StreamLimits};

/// The replica of `folder` (a path in the app folder, `Photos/Originals`; empty for the app
/// folder itself) of the account the grant covers, over relays accountd opens to `endpoint`.
pub fn graph_replica<T: Transport>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    folder: &str,
    clock: Clock,
) -> Result<GraphReplica<StreamHttp<RelayDial<T>>>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let dial = RelayDial::new(accounts, grant, endpoint);
    Ok(GraphReplica::new(
        StreamHttp::new(dial, StreamLimits::default()),
        &base,
        folder,
        clock,
    ))
}
