//! Where a replica's change feed was last read.

use serde::{Deserialize, Serialize};

/// An opaque change-feed position: a Drive page token, a Graph delta link, a Dropbox cursor, a
/// WebDAV sync-token, or the head of our own manifest for S3. Only the replica reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Anchor(pub String);

/// Where to read changes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Cursor {
    /// The beginning: a full listing of what exists now, without tombstones.
    Start,
    /// After this anchor.
    At(Anchor),
}
