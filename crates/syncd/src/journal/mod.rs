//! The SQLite journal of one dataset of one account (design/31 §6.1): items, the anchor,
//! tombstones and conflicts, in one file under `$XDG_STATE_HOME/porter/sync/<account>/`.
//!
//! Every change is an [`Op`] list applied in one transaction (`apply`), so the engine's steps
//! are the unit of a crash: either a step's rows are all there or none are. The rows are
//! porter-sync's values (`porter_sync::journal`); this file only stores them.

mod rows;
mod schema;

pub use schema::SCHEMA_VERSION;

use porter_sync::{
    ItemPath, JournalItem, LocalId, RemoteId, StoredAnchor, StoredConflict, StoredTombstone,
};
use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Why the journal could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// SQLite refused.
    #[error("journal: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The file's directory could not be made.
    #[error("journal: {0}")]
    Io(#[from] std::io::Error),
    /// The file was written by a newer syncd.
    #[error("journal: schema version {found} is newer than this syncd's")]
    Newer {
        /// Its version.
        found: i64,
    },
    /// A row holds something this syncd cannot read.
    #[error("journal: {0}")]
    Corrupt(String),
    /// A test cut the process off before this write.
    #[cfg(test)]
    #[error("journal: cut before write")]
    Cut,
}

/// One change to the journal. A list of them commits together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Inserts the item, or replaces the row with its local id.
    PutItem(JournalItem),
    /// Removes the item.
    DeleteItem(LocalId),
    /// Stores where the feed stands.
    SetAnchor(StoredAnchor),
    /// Forgets the anchor (it expired).
    ClearAnchor,
    /// Stores a tombstone, or replaces the one for that id.
    PutTombstone(StoredTombstone),
    /// Marks the tombstone of that id acknowledged.
    Acknowledge(RemoteId),
    /// Drops every acknowledged tombstone.
    CompactTombstones,
    /// Stores a conflict.
    AddConflict(StoredConflict),
    /// Removes a stored conflict by number.
    DropConflict(i64),
}

/// A journal file, open.
#[derive(Debug)]
pub struct Journal {
    connection: Mutex<Connection>,
    writes: AtomicUsize,
    #[cfg(test)]
    cut: Mutex<Option<usize>>,
}

impl Journal {
    /// Opens the journal at `path`, creating it (and its directory) and migrating it.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        Self::open_with(path, schema::MIGRATIONS)
    }

    pub(crate) fn open_with(path: &Path, migrations: &[&str]) -> Result<Self, JournalError> {
        Self::open_synchronous(path, migrations, "FULL")
    }

    /// A journal whose commits are not flushed to the disk: for the tests that make thousands of
    /// commits (the thousand photos), which prove the engine, not the disk. The crash tests keep
    /// `open` (`cut_at` stops a write before it commits, whatever the flush).
    #[cfg(test)]
    pub(crate) fn open_unflushed(path: &Path) -> Result<Self, JournalError> {
        Self::open_synchronous(path, schema::MIGRATIONS, "OFF")
    }

    fn open_synchronous(
        path: &Path,
        migrations: &[&str],
        synchronous: &str,
    ) -> Result<Self, JournalError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut connection = Connection::open(path)?;
        connection.execute_batch(&format!("PRAGMA synchronous = {synchronous};"))?;
        schema::migrate(&mut connection, migrations)?;
        Ok(Self {
            connection: Mutex::new(connection),
            writes: AtomicUsize::new(0),
            #[cfg(test)]
            cut: Mutex::new(None),
        })
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        // A panic mid-statement leaves SQLite's own transaction to roll back.
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// How many transactions this handle has committed (a test counts the cut points).
    pub fn writes(&self) -> usize {
        self.writes.load(Ordering::Relaxed)
    }

    /// Makes the `n`th write from now (0 is the next) fail before it commits, and every one
    /// after it: the process died there.
    #[cfg(test)]
    pub(crate) fn cut_at(&self, n: usize) {
        *self.cut.lock().unwrap_or_else(PoisonError::into_inner) = Some(self.writes() + n);
    }

    /// The schema version of the file.
    pub fn version(&self) -> Result<i64, JournalError> {
        Ok(self.connection().query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?)
    }

    /// Commits `ops` together.
    pub fn apply(&self, ops: &[Op]) -> Result<(), JournalError> {
        #[cfg(test)]
        {
            let cut = *self.cut.lock().unwrap_or_else(PoisonError::into_inner);
            if cut.is_some_and(|at| self.writes() >= at) {
                return Err(JournalError::Cut);
            }
        }
        let mut connection = self.connection();
        let tx = connection.transaction()?;
        for op in ops {
            rows::apply(&tx, op)?;
        }
        tx.commit()?;
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Every item, by local id.
    pub fn items(&self) -> Result<Vec<JournalItem>, JournalError> {
        rows::items(&self.connection())
    }

    /// The item the replica knows as `id`.
    pub fn item_by_remote(&self, id: &RemoteId) -> Result<Option<JournalItem>, JournalError> {
        rows::item_where(&self.connection(), "remote_id = ?1", &id.0)
    }

    /// The item waiting to be uploaded for the first time at `path` (no remote id yet).
    pub fn new_item_at(&self, path: &ItemPath) -> Result<Option<JournalItem>, JournalError> {
        rows::item_where(
            &self.connection(),
            "remote_id IS NULL AND state = 'pending_upload' AND path = ?1",
            &path.0,
        )
    }

    /// The tombstone kept for `id`, if any.
    pub fn tombstone_of(&self, id: &RemoteId) -> Result<Option<StoredTombstone>, JournalError> {
        Ok(self
            .tombstones()?
            .into_iter()
            .find(|stored| stored.tombstone.id == *id))
    }

    /// The anchor, if one is stored.
    pub fn anchor(&self) -> Result<Option<StoredAnchor>, JournalError> {
        rows::anchor(&self.connection())
    }

    /// Every tombstone, by remote id.
    pub fn tombstones(&self) -> Result<Vec<StoredTombstone>, JournalError> {
        rows::tombstones(&self.connection())
    }

    /// Every stored conflict, oldest first.
    pub fn conflicts(&self) -> Result<Vec<StoredConflict>, JournalError> {
        rows::conflicts(&self.connection())
    }
}

#[cfg(test)]
mod tests;
