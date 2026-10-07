//! The Google Drive replica (design/31 §6.1, lane google-drive): [`GdriveReplica`] implements
//! porter-sync's `Replica` over a folder of Drive's app data folder (`spaces=appDataFolder`, the
//! scope `drive.appdata`, which is the only Drive scope this build asks).
//!
//! - **Changes** are `changes.list` with a page token as the anchor (`changes:<token>`). The
//!   first listing is `files.list` of the space, page by page, after the start token was taken
//!   (so nothing between is missed): while it runs the anchor is `list:<start>:<page>`, and the
//!   last page's anchor is `changes:<start>`. A token Drive refuses (`400`, `404`, `410` on a
//!   resumed anchor) is `AnchorExpired`. A change names a file by id and parent id, so paths are
//!   built from the folders seen (and asked for when one is not).
//! - **Writes** are checked first: a new file looks for its name in the parent (a name already
//!   there is a `Conflict`: Drive allows two files of one name, this replica does not make
//!   them), a changed one or a removal reads the file's `version` and compares it with the base
//!   (a stale base is a `Conflict`, never an overwrite). Drive documents no conditional update,
//!   so another writer can still slip in between the check and the write: the window is one
//!   round trip (FINDINGS). A file up to [`SIMPLE_MAX`] bytes is one multipart request; a larger
//!   one goes in a resumable session, in chunks of a multiple of [`CHUNK_UNIT`] (256 KiB).
//! - **Quota** is `about`'s `storageQuota` (`usage`, `limit`).
//! - **The connection** is whatever [`porter_http::Http`] the replica is handed. In syncd it is
//!   `storage_webdav::StreamHttp` over the descriptors of accountd's `Tokens.OpenAuthenticated`,
//!   whose relay adds the bearer, so syncd never holds a token. The upload host
//!   (`/upload/drive/v3`) and a resumable session's URL are the same origin as the API, so one
//!   relay serves all; a session URL on another origin is refused.
//! - **Version** is Drive's `version`, which grows with every change to the file (a rename too).
//!   There is no ETag to keep.
//!
//! The crate is pure: no runtime and no socket. It reuses storage-webdav's `StreamHttp`, `Dial`
//! and `Clock` (nothing of it changed) and keeps its own error classes and wire reading. Every
//! endpoint, field and status is written from Google's documentation, without network access:
//! unverified (FINDINGS).

mod addr;
mod feed;
mod json;
mod refuse;
mod replica;
mod upload;
mod write;

pub use replica::{CHUNK_UNIT, GdriveReplica, SIMPLE_MAX, Uploads};
pub use storage_webdav::{Clock, DELETED, Dial, StreamHttp, StreamLimits};
