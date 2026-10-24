//! A whole drive, listed: every file, folder and online document of a Storage account's drive
//! (OneDrive's, or My Drive), kept by id in a journal of its own (`storage_drive`) for Files to
//! show and, in S2, for the file system to serve. Only listed: no byte is downloaded here, and
//! it does not go through the eager [`crate::engine::Engine`], which downloads every file whole.
//!
//! - [`DriveJournal`]: the entries (id, parent, name, kind, size, hash, version, modified), the
//!   anchor and the replica's tree, one transaction per page; paths are built when read, so a
//!   folder moved re-paths its children with no listing again. The read API S2 serves from:
//!   [`DriveJournal::children`], [`DriveJournal::entry_at`], [`DriveJournal::entry`].
//! - [`list_once`]: one pass of the feed into the journal. An expired anchor lists the drive
//!   again and removes what the new listing lacks. The engine's hold on a mass delete is not
//!   kept: it guards local copies a listing would discard, and a listing holds none (a wrong
//!   listing is mended by the next one), and it waits for the person, whom the drive has no way
//!   to ask yet. S2, which keeps downloads, decides what guards them.
//! - [`keep_listing`]: the pass now and then, as the storage supervisor runs it for each
//!   account syncd holds a grant of the whole drive for (`StorageKind::Drive`, made by the
//!   person's "Show in Files"). The grant going ends it and keeps the journal, so turning it on
//!   again resumes from the anchor.

mod files;
mod journal;
mod listing;

pub use files::{
    CacheKnob, DriveEvent, DriveSet, Kept, LiveDrive, SIGN_IN_WORDS, UNREADABLE_WORDS,
};
pub use journal::{Applied, DriveEntry, DriveJournal};
pub use listing::{DriveReplica, Listed, Pass, keep_listing, keep_listing_told, list_once};

use crate::paths::{AccountDir, Paths};

/// The dataset's slug: its journal is `storage_drive.sqlite` beside the account's others.
pub const DRIVE_DATASET: &str = "storage_drive";

/// Where the drive journal of `account` is.
pub fn journal_path(paths: &Paths, account: &AccountDir) -> std::path::PathBuf {
    paths.journal(account, DRIVE_DATASET)
}
