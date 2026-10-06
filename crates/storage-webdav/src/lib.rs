//! The WebDAV replica (design/31 §6.1, porter PLAN W6b): [`WebDavReplica`] implements
//! porter-sync's `Replica` over one folder of a WebDAV server (Nextcloud, ownCloud, a plain DAV
//! share) through porter-dav's parsers and porter-http's seam.
//!
//! - **Changes** are a sync-collection report (RFC 6578) with the sync-token as the anchor, or,
//!   where the server has none, a walk of the tree diffed by etag. The server's token-expired
//!   answer is `AnchorExpired`.
//! - **Writes** carry their base as `If-Match` (an existing item) or `If-None-Match: *` (a new
//!   one), so a stale base is a `Conflict` read back from the server and never an overwrite.
//! - **Quota** is RFC 4331's two properties.
//! - **The connection** is whatever [`porter_http::Http`] the replica is handed. [`StreamHttp`]
//!   is one over authenticated byte streams: in syncd each is the descriptor of accountd's
//!   `Tokens.OpenAuthenticated`, whose relay adds the credential, so syncd never holds one.
//!
//! The crate is pure: no runtime and no socket (a [`Dial`] is the host's).

mod clock;
mod entry;
mod feed;
mod path;
mod refuse;
mod replica;
mod requests;
mod stream_http;
mod wire;
mod write;

pub use clock::Clock;
pub use replica::{DELETED, WebDavReplica};
pub use stream_http::{Dial, StreamHttp, StreamLimits};
