//! The Microsoft Graph replica (design/31 §6.1, porter PLAN W6c): [`GraphReplica`] implements
//! porter-sync's `Replica` over a folder of OneDrive's app folder (`/me/drive/special/approot`).
//!
//! - **Changes** are a delta query with the delta link as the anchor; a `410` (`resyncRequired`)
//!   is `AnchorExpired`. Delta items carry ids and parent ids and often no path, so paths are
//!   built from the folders seen (and asked for when one is not).
//! - **Writes** are conditional: a new item is created with `@microsoft.graph.conflictBehavior=fail`,
//!   a changed one or a removal carries `If-Match` with the eTag the writer saw, so a stale base
//!   is a `Conflict` read back from the server and never an overwrite. A file up to
//!   [`SIMPLE_MAX`] bytes (Graph's 4 MB simple-upload limit) is one PUT; a larger one goes in an
//!   upload session, in chunks of a multiple of [`CHUNK_UNIT`] (320 KiB).
//! - **Quota** is the drive's `quota` (`used`, `total`).
//! - **The connection** is whatever [`porter_http::Http`] the replica is handed. In syncd it is a
//!   [`Routed`]: `porter_http::stream::StreamHttp` over the descriptors of accountd's
//!   `Tokens.OpenAuthenticated` (whose relay adds the bearer, so syncd never holds a token) for
//!   the Graph host, and over those of `Tokens.OpenLinked` (no credential) for the hosts of an
//!   `uploadUrl` and of the redirect a download answers with.
//!
//! The crate is pure: no runtime and no socket. It reuses porter-http's stream client (`StreamHttp`,
//! `Dial`) and storage-webdav's `Clock` (nothing of it changed) and keeps its own error classes
//! and wire reading.

mod addr;
mod feed;
mod json;
mod refuse;
mod replica;
mod route;
mod upload;
mod write;

pub use porter_http::stream::{Dial, StreamHttp, StreamLimits};
pub use replica::{CHUNK_UNIT, GraphReplica, SIMPLE_MAX, Uploads};
pub use route::Routed;
pub use storage_webdav::{Clock, DELETED};
