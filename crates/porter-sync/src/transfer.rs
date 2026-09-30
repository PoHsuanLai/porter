//! What a read fetches and a write sends.

use crate::item::{ContentHash, ItemPath, RemoteId};
use porter_core::Bytes;

/// Item content. Not stored or sent as a value of ours, so it has no serde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob(pub Vec<u8>);

/// Which bytes of an item to fetch (a partial download where `ranges` is offered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// All of it.
    Whole,
    /// `len` bytes from `start`.
    Span {
        /// The first byte.
        start: Bytes,
        /// How many.
        len: Bytes,
    },
}

/// Which item a write goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutTarget {
    /// A new item at this path.
    New(ItemPath),
    /// An existing item.
    Existing(RemoteId),
}

/// One write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutItem {
    /// Where.
    pub target: PutTarget,
    /// The content.
    pub content: Blob,
    /// Its hash, so a replica that reports hashes can skip known content.
    pub hash: Option<ContentHash>,
}
