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
#[non_exhaustive]
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
    /// The account's sign-in was refused: nothing more is sent until it is signed in again.
    #[error("unauthorized")]
    Unauthorized,
    /// The item is in use on the replica (locked there): try again after the wait.
    #[error("in use; retry after {} s", .0.0)]
    InUse(RetryAfter),
    /// The replica does not take the item's name or path.
    #[error("name refused")]
    NameRefused,
    /// The replica refused the change for a reason of its own, which no name taken explains.
    #[error("declined")]
    Declined,
}

/// Why an online document could not be exported as a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExportError {
    /// The backend exports no file of it (a Form, a Site, a Map, or a backend with no export).
    #[error("no file format")]
    Unsupported,
    /// The document is larger than the backend exports.
    #[error("too large to export")]
    TooLarge,
    /// The export could not be read.
    #[error(transparent)]
    Read(#[from] ReplicaError),
}

/// Why a read failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
    /// The account has not given the access the replica needs yet (the whole of a drive, asked
    /// when first used): nothing is retried until the person gives it.
    #[error("access not given yet")]
    NeedsConsent,
}
