//! How a replica refuses: conflicts are values the journal stores, never hidden overwrites.

use crate::item::{BaseVersion, RemoteId, RemoteVersion};
use serde::{Deserialize, Serialize};

/// A write whose base is not the replica's current version. The journal keeps it as an object
/// `{item, base, local, remote}` (the local side is the journal's own) and the owning app
/// resolves it by its dataset's rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Conflict {
    /// The item.
    pub item: RemoteId,
    /// What the write was based on.
    pub base: BaseVersion,
    /// What the replica has.
    pub remote: RemoteSide,
}

/// The replica's side of a conflict.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum RemoteSide {
    /// Changed to this version.
    Changed(RemoteVersion),
    /// Deleted, at this version.
    Deleted(RemoteVersion),
    /// Created by someone else while the writer believed it absent.
    Exists(RemoteId, RemoteVersion),
}

/// Seconds to wait before retrying (`Retry-After`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RetryAfter(pub u32);

/// Why a write was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PutRefused {
    /// The base is stale.
    #[error("conflict on {:?}", .0.item)]
    Conflict(Conflict),
    /// The store is full.
    #[error("quota exceeded")]
    Quota,
    /// The account may not write there.
    #[error("forbidden")]
    Forbidden,
    /// Try again later.
    #[error("transient; retry after {} s", .0.0)]
    Transient(RetryAfter),
}

/// Why a read failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReplicaError {
    /// The anchor is no longer valid (Dropbox 409, Graph resync, an expired page token):
    /// list from `Cursor::Start` and reconcile by content hash, uploading nothing known.
    #[error("anchor expired")]
    AnchorExpired,
    /// The account's token was refused.
    #[error("unauthorized")]
    Unauthorized,
    /// Try again later.
    #[error("transient; retry after {} s", .0.0)]
    Transient(RetryAfter),
    /// The item or the dataset folder is gone.
    #[error("gone")]
    Gone,
}
