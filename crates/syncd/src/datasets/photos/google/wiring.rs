//! The HTTP of Google Photos as syncd runs it: relay streams from accountd's
//! `Tokens.OpenAuthenticated` (the relay adds the bearer, so syncd never holds the Google
//! token), one dial per endpoint of the grant. The upload API and the Picker API are two rows of
//! the provider file on two origins (`photoslibrary.googleapis.com`,
//! `photospicker.googleapis.com`).
//!
//! A picked item's `baseUrl` is on a third host. The picker's `Http` sends the Picker API's
//! origin through the authenticated relay and any other origin through `OpenLinked`, which adds
//! no credential and which accountd opens only for an origin the provider file declares. Google
//! wants the bearer on those downloads, so against the real service they fail until accountd can
//! open an *authenticated* relay to an extra origin of a row (FINDINGS: interface ask); against
//! the fake, whose items are on the Picker origin, they work.

use super::picker::PhotosPicker;
use crate::graph::{LinkedDial, bare_url};
use crate::webdav::{BuildError, RelayDial};
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, Origin, WebUrl};
use std::sync::Arc;
use storage_graph::Routed;
use storage_webdav::{StreamHttp, StreamLimits};

/// What the upload API sends through.
pub type LibraryHttp<T> = StreamHttp<RelayDial<T>>;

type MakeLinked<T> = Box<dyn Fn(&Origin) -> StreamHttp<LinkedDial<T>> + Send + Sync>;

/// What the Picker sends through: its origin over the authenticated relay, any other over a
/// linked one.
pub type PickerHttp<T> = Routed<StreamHttp<RelayDial<T>>, StreamHttp<LinkedDial<T>>, MakeLinked<T>>;

/// The Picker API as syncd holds it for an account.
pub type GooglePicker<T> = PhotosPicker<PickerHttp<T>>;

/// The client for the Photos upload API at `endpoint`, under `grant`.
pub fn library_http<T: Transport>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
) -> LibraryHttp<T> {
    StreamHttp::new(
        RelayDial::new(accounts, grant, endpoint),
        StreamLimits::default(),
    )
}

/// The client for the Picker API at `endpoint`, under `grant`.
pub fn picker_http<T: Transport + 'static>(
    accounts: Arc<Accounts<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
) -> Result<PickerHttp<T>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let home = StreamHttp::new(
        RelayDial::new(Arc::clone(&accounts), grant.clone(), endpoint),
        StreamLimits::default(),
    );
    let linked: MakeLinked<T> = Box::new(move |origin| {
        StreamHttp::new(
            LinkedDial::new(Arc::clone(&accounts), grant.clone(), bare_url(origin)),
            StreamLimits::default(),
        )
    });
    Ok(Routed::new(home, &base, linked))
}
