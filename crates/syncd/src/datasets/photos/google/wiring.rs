//! The HTTP of Google Photos as syncd runs it: relay streams from accountd's
//! `Tokens.OpenAuthenticated` (the relay adds the bearer, so syncd never holds the Google
//! token), one dial per endpoint of the grant. The upload API and the Picker API are two rows of
//! the provider file on two origins (`photoslibrary.googleapis.com`,
//! `photospicker.googleapis.com`).
//!
//! A picked item's `baseUrl` is on a third host (`lh3.googleusercontent.com`) that wants the
//! bearer. The picker's `Http` sends every origin through `OpenAuthenticated`: the Picker API's
//! own, and any other as an origin of the grant. accountd relays WITH the bearer only to an
//! origin the provider file names in the picker row's `auth_origins` (exact hosts, the row's
//! scheme) and refuses every other, so a `baseUrl` on an unlisted host fails to dial.

use super::picker::PhotosPicker;
use crate::graph::bare_url;
use crate::webdav::{BuildError, RelayDial};
use porter_client::{Accounts, Transport};
use porter_core::{EndpointUrl, GrantId, Origin, WebUrl};
use std::sync::Arc;
use storage_graph::Routed;
use storage_webdav::{StreamHttp, StreamLimits};

/// What the upload API sends through.
pub type LibraryHttp<T> = StreamHttp<RelayDial<T>>;

type MakeAuth<T> = Box<dyn Fn(&Origin) -> StreamHttp<RelayDial<T>> + Send + Sync>;

/// What the Picker sends through: every origin over an authenticated relay, the Picker's own
/// and any other the provider file names.
pub type PickerHttp<T> = Routed<StreamHttp<RelayDial<T>>, StreamHttp<RelayDial<T>>, MakeAuth<T>>;

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
    let fallback = endpoint.clone();
    let home = StreamHttp::new(
        RelayDial::new(Arc::clone(&accounts), grant.clone(), endpoint),
        StreamLimits::default(),
    );
    let linked: MakeAuth<T> = Box::new(move |origin| {
        StreamHttp::new(
            RelayDial::new(
                Arc::clone(&accounts),
                grant.clone(),
                bare_url(origin).unwrap_or_else(|| fallback.clone()),
            ),
            StreamLimits::default(),
        )
    });
    Ok(Routed::new(home, &base, linked))
}
