//! The sync engine contract (design/31 §6): one small [`Replica`] trait per backend family,
//! opaque anchors, a base version on every write, conflicts and tombstones as values. Pure;
//! the SQLite journal and the scheduler are syncd's; the journal's rows and pure rules
//! (`journal`, `journal_reconcile`) are here.
//!
//! The rules are plain functions over values. Each dataset settles a conflict its own way, and a
//! listing that would not discard anything is not a mass delete:
//!
//! ```
//! use porter_sync::{ConflictRule, DatasetKind, mass_delete};
//!
//! assert_eq!(DatasetKind::Files.conflict_rule(), ConflictRule::KeepBoth);
//! assert_eq!(DatasetKind::PhotosOriginals.conflict_rule(), ConflictRule::Impossible);
//! assert_eq!(mass_delete(&[], &[]), None);
//! ```

mod anchor;
mod change;
mod clock;
mod dataset;
mod drive_write;
mod folders;
mod item;
mod journal;
mod journal_reconcile;
#[cfg(feature = "testing")]
mod memory;
mod quota;
mod refusal;
mod replica;
pub mod status;
mod transfer;

pub use anchor::{Anchor, Cursor};
pub use change::{Change, ChangePage, DELETED, More, Tombstone};
pub use clock::Clock;
pub use dataset::{ConflictRule, DatasetKind};
pub use drive_write::{
    Content, DriveWrite, Found, Place, Progress, Session, Unheard, Upload, UploadTo, Written,
};
pub use folders::{Folder, Folders, Held};
pub use item::{BaseVersion, ContentHash, ItemKind, ItemPath, RemoteId, RemoteItem, RemoteVersion};
pub use journal::{
    Acknowledgement, ItemState, JournalItem, LocalId, Resolution, StoredAnchor, StoredConflict,
    StoredTombstone, TombstoneOrigin,
};
pub use journal_reconcile::{
    LocalChange, MASS_DELETE_FLOOR, MassDelete, Scanned, local_changes, mass_delete, reconcile,
};
#[cfg(feature = "testing")]
pub use memory::MemoryReplica;
pub use quota::Quota;
pub use refusal::{Conflict, ExportError, PutRefused, RemoteSide, ReplicaError, RetryAfter};
pub use replica::Replica;
pub use transfer::{Blob, ByteRange, PutItem, PutTarget};
