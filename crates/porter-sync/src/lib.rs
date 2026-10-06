//! The sync engine contract (design/31 §6): one small [`Replica`] trait per backend family,
//! opaque anchors, a base version on every write, conflicts and tombstones as values. Pure;
//! the SQLite journal and the scheduler are syncd's; the journal's rows and pure rules
//! (`journal`, `journal_reconcile`) are here.

mod anchor;
mod change;
mod dataset;
mod item;
mod journal;
mod journal_reconcile;
#[cfg(feature = "testing")]
mod memory;
mod quota;
mod refusal;
mod replica;
mod transfer;

pub use anchor::{Anchor, Cursor};
pub use change::{Change, ChangePage, More, Tombstone};
pub use dataset::{ConflictRule, DatasetKind};
pub use item::{BaseVersion, ContentHash, ItemPath, RemoteId, RemoteItem, RemoteVersion};
pub use journal::{
    Acknowledgement, ItemState, JournalItem, LocalId, Resolution, StoredAnchor, StoredConflict,
    StoredTombstone, TombstoneOrigin,
};
pub use journal_reconcile::{LocalChange, Scanned, local_changes, reconcile};
#[cfg(feature = "testing")]
pub use memory::MemoryReplica;
pub use quota::Quota;
pub use refusal::{Conflict, PutRefused, RemoteSide, ReplicaError, RetryAfter};
pub use replica::Replica;
pub use transfer::{Blob, ByteRange, PutItem, PutTarget};
