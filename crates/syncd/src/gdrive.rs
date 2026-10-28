//! The Google Drive replica as syncd runs it: one authenticated relay stream per connection from
//! accountd's `Tokens.OpenAuthenticated` ([`RelayDial`]), so syncd never holds the
//! Google token. The relay adds `Authorization: Bearer` and reaches the endpoint's origin
//! (`https://www.googleapis.com`), which serves the API (`/drive/v3`) and the upload host
//! (`/upload/drive/v3`) alike: a resumable session's URL is on the same origin, so no second
//! relay and no linked origin is needed (unlike Graph, whose upload URLs are on other hosts).
//! [`gdrive_replica`] builds the replica of a dataset's folder in the account's app data folder.

use crate::webdav::BuildError;
use porter_client::{Grants, GrantsLink};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_http_relay::{RelayDial, RelayHttp, RelayLimits};
use porter_ids::WebUrl;
use porter_sync::Clock;
use std::sync::Arc;
use storage_gdrive::GdriveReplica;

/// The replica of `folder` (a path in the app data folder, `Photos/Originals`; empty for the
/// folder itself) of the account the grant covers, over relays accountd opens to `endpoint`
/// (the `google_drive` row's, `https://www.googleapis.com/drive/v3`).
pub fn gdrive_replica<T: GrantsLink>(
    accounts: Arc<Grants<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    folder: &str,
    clock: Clock,
) -> Result<GdriveReplica<RelayHttp<RelayDial<T>>>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let dial = RelayDial::authenticated(accounts, grant, endpoint);
    Ok(GdriveReplica::new(
        RelayHttp::new(dial, RelayLimits::default()),
        &base,
        folder,
        clock,
    ))
}
