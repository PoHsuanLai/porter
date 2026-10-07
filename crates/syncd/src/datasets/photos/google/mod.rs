//! Google Photos as syncd carries it (design/31 R1: Photos is an upload target and a picker
//! import, never a library mirror; the build asks only the scopes `photoslibrary.appendonly` and
//! `photospicker.mediaitems.readonly`). Behind the same `SYNCD_PHOTOS` switch as the Graph
//! Photos datasets, for a Photos grant on a Google account.
//!
//! - **Upload** (`upload`, `ledger`, `api`): the dataset `google_photos_upload`. The files
//!   dropped into `$XDG_DATA_HOME/porter/photos/<account>/upload/` are sent to an album the app
//!   created (`albums.create`, then per file the bytes to `uploads` and the upload token to
//!   `mediaItems:batchCreate`). It pulls nothing: the replica's feed is always empty, and a
//!   local delete is not a delete in Google Photos (the scope could not do it). A file is
//!   never sent twice: the ledger keeps the SHA-256 of everything sent, so the same bytes under
//!   another name, or the same file after a restart, are not uploaded again.
//! - **Picker** (`picker`): [`PhotosPicker`], the typed API the Photos app calls. `start` opens a
//!   Picker session and answers the `pickerUri` the app opens in the browser; `poll` says
//!   whether the person has finished picking; `import` lists the picked items, downloads them
//!   into `$XDG_DATA_HOME/porter/photos/<account>/picked/<session>/` and deletes the session.
//!   Nothing outside what the person picked is read.
//! - **Wiring** (`wiring`): the HTTP of both over accountd's `OpenAuthenticated` relays (the
//!   Photos upload and Picker endpoints are two rows of the provider file, two origins), so
//!   syncd never holds the Google token.
//!
//! Every endpoint, field and status is written from Google's documentation, without network
//! access: unverified (FINDINGS).

mod api;
mod ledger;
mod picker;
mod upload;
mod wiring;

pub use api::{AlbumId, ApiError, MediaId, PhotosApi, UploadToken, mime_of};
pub use ledger::{Entry, Ledger, LedgerError};
pub use picker::{
    Imported, PhotosPicker, PickerError, PickerSession, PickerState, SessionEnd, SessionId,
};
pub use upload::{ALBUM_TITLE, SLUG, UploadReplica};
pub use wiring::{GooglePicker, LibraryHttp, PickerHttp, library_http, picker_http};
