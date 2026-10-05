//! A sync-collection answer (RFC 6578).

use crate::multistatus::{DavFault, Multistatus};

/// One change a sync-collection report lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncChange {
    /// Created or changed, with its etag.
    Changed {
        /// The resource.
        href: String,
        /// Its etag.
        etag: String,
    },
    /// Removed (a 404 response).
    Removed {
        /// The resource.
        href: String,
    },
}

/// The changes since a token, and the token for next time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReply {
    /// What changed.
    pub changes: Vec<SyncChange>,
    /// The new token; absent when the server expired ours (`valid-sync-token`), which the
    /// consumer treats as an expired anchor.
    pub sync_token: Option<String>,
}

/// Reads the changes and the token out of a multistatus.
pub fn parse_sync_collection(status: &Multistatus) -> Result<SyncReply, DavFault> {
    let _ = status;
    todo!("responses with a 404 status are removals; the `sync-token` element is the next token")
}
