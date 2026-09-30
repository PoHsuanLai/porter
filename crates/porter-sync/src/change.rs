//! The change feed: pages of upserts and tombstones, each page ending in the next anchor.

use crate::anchor::Anchor;
use crate::item::{RemoteId, RemoteItem, RemoteVersion};
use porter_core::UnixSeconds;
use serde::{Deserialize, Serialize};

/// One change.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Change {
    /// Created or changed.
    Upsert(RemoteItem),
    /// Deleted.
    Tombstone(Tombstone),
}

/// A deletion, kept with its time until every replica has acknowledged it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tombstone {
    /// The item deleted.
    pub id: RemoteId,
    /// The version the deletion made.
    pub version: RemoteVersion,
    /// When.
    pub deleted_at: UnixSeconds,
}

/// Whether a page is the last one for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum More {
    /// Ask again from `next` at once.
    More,
    /// Caught up; ask again from `next` at the next poll or push.
    Done,
}

/// One page of the feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePage {
    /// The changes, oldest first.
    pub changes: Vec<Change>,
    /// Where the next read starts; store it only once the page is applied.
    pub next: Anchor,
    /// Whether more pages follow.
    pub more: More,
}
