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
use crate::webdav::BuildError;
use porter_client::{Grants, GrantsLink};
use porter_core::EndpointUrl;
use porter_core::GrantId;
use porter_http_relay::{RelayDial, RelayHttp, RelayLimits, Routed, bare_url};
use porter_ids::{Origin, WebUrl};
use std::sync::Arc;

/// What the upload API sends through.
pub type LibraryHttp<T> = RelayHttp<RelayDial<T>>;

type MakeAuth<T> = Box<dyn Fn(&Origin) -> RelayHttp<RelayDial<T>> + Send + Sync>;

/// What the Picker sends through: every origin over an authenticated relay, the Picker's own
/// and any other the provider file names.
pub type PickerHttp<T> = Routed<RelayHttp<RelayDial<T>>, RelayHttp<RelayDial<T>>, MakeAuth<T>>;

/// The Picker API as syncd holds it for an account.
pub type GooglePicker<T> = PhotosPicker<PickerHttp<T>>;

/// The client for the Photos upload API at `endpoint`, under `grant`.
pub fn library_http<T: GrantsLink>(
    accounts: Arc<Grants<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
) -> LibraryHttp<T> {
    RelayHttp::new(
        RelayDial::authenticated(accounts, grant, endpoint),
        RelayLimits::default(),
    )
}

/// The client for the Picker API at `endpoint`, under `grant`.
pub fn picker_http<T: GrantsLink + 'static>(
    accounts: Arc<Grants<T>>,
    grant: GrantId,
    endpoint: EndpointUrl,
) -> Result<PickerHttp<T>, BuildError> {
    let base = WebUrl::try_from(&endpoint).map_err(|_| BuildError::NotWeb)?;
    let fallback = endpoint.clone();
    let home = RelayHttp::new(
        RelayDial::authenticated(Arc::clone(&accounts), grant.clone(), endpoint),
        RelayLimits::default(),
    );
    let linked: MakeAuth<T> = Box::new(move |origin| {
        RelayHttp::new(
            RelayDial::authenticated(
                Arc::clone(&accounts),
                grant.clone(),
                bare_url(origin).unwrap_or_else(|| fallback.clone()),
            ),
            RelayLimits::default(),
        )
    });
    Ok(Routed::new(home, &base, linked))
}
