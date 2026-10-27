//! The Microsoft Graph replica (design/31 §6.1, porter PLAN W6c): [`GraphReplica`] implements
//! porter-sync's `Replica` over a folder of OneDrive's app folder (`/me/drive/special/approot`)
//! or, given [`Root::Drive`], of the whole drive (`/me/drive/root`).
//!
//! - **Changes** are a delta query with the delta link as the anchor; a `410` (`resyncRequired`)
//!   is `AnchorExpired`. Delta items carry ids and parent ids and often no path, so paths are
//!   built from the folders seen (and asked for when one is not). The folders seen are a
//!   [`porter_sync::Folders`] a dataset may keep ([`GraphReplica::folders`]) and hand back
//!   ([`GraphReplica::with_folders`]). An app folder's feed is its files; a whole drive's is its
//!   folders too.
//! - **Writes** are conditional: a new item is created with `@microsoft.graph.conflictBehavior=fail`,
//!   a changed one or a removal carries `If-Match` with the eTag the writer saw, so a stale base
//!   is a `Conflict` read back from the server and never an overwrite. A file up to
//!   [`SIMPLE_MAX`] bytes (Graph's 4 MB simple-upload limit) is one PUT; a larger one goes in an
//!   upload session, in chunks of a multiple of [`CHUNK_UNIT`] (320 KiB).
//! - **Whole-drive writes** ([`porter_sync::DriveWrite`], for a replica of [`Root::Drive`]): a
//!   folder or a file made in a parent by id, new content for an item by id, a rename or move
//!   as a `PATCH`, the recycle bin as a `DELETE`, each with `If-Match`; a large file in an
//!   upload session that a new replica resumes; the names OneDrive refuses ([`accepts_name`]).
//! - **Quota** is the drive's `quota` (`used`, `total`).
//! - **The connection** is whatever [`porter_http::Http`] the replica is handed. In syncd it is a
//!   `porter_http_relay::Routed`: a `porter_http_relay::RelayHttp` over the relays of accountd's
//!   `Tokens.OpenAuthenticated` (whose relay adds the bearer, so syncd never holds a token) for
//!   the Graph host, and over those of `Tokens.OpenLinked` (no credential) for the hosts of an
//!   `uploadUrl` and of the redirect a download answers with.
//!
//! The crate is pure: no runtime and no socket. It keeps its own vendor error reasons and wire reading on top of porter-sync's
//! `status` and dates its tombstones with porter-sync's `Clock`.

mod addr;
mod drive;
mod feed;
mod json;
mod refuse;
mod replica;
mod upload;
mod write;

pub use addr::Root;
pub use drive::accepts_name;
pub use replica::{CHUNK_UNIT, GraphReplica, SIMPLE_MAX, Uploads};
