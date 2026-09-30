//! A remote item and the versions a write is checked against.

use porter_core::Bytes;
use serde::{Deserialize, Serialize};

/// A replica's own id for an item (a Drive file id, a WebDAV href).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemoteId(pub String);

/// A replica's version of an item (an etag, a revision id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RemoteVersion(pub String);

/// A path inside the dataset's folder (`Originals/2026/09/IMG_1234.HEIC`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemPath(pub String);

/// A content hash in the form the replica reports it (design/31 `HashKind`), hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub String);

/// The version a write was based on. A mismatch is a conflict, never an overwrite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum BaseVersion {
    /// The writer believes the item does not exist yet.
    Absent,
    /// The writer last saw this version.
    At(RemoteVersion),
}

/// One live item as the change feed reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemoteItem {
    /// Its id.
    pub id: RemoteId,
    /// Its current version.
    pub version: RemoteVersion,
    /// Where it is.
    pub path: ItemPath,
    /// Its size.
    pub size: Bytes,
    /// Its content hash, when the replica reports one.
    pub hash: Option<ContentHash>,
}
