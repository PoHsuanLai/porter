//! [`PimMirror`]: the local side of one collection, a directory of the vdir.
//!
//! **Read-only.** The engine is two-way: it diffs [`Dataset::scan`] against its journal and
//! uploads what differs. This dataset reports what *it stored* (its ledger, seeded from the
//! journal when opened), never what is on disk, so an edit, a new file or a deletion made in the
//! vdir looks like no change at all and nothing is uploaded or removed on the server. The v1
//! rule: a local change does not survive. A small item (up to [`ITEM_CAP`] bytes, [`TOTAL_CAP`]
//! in all) is kept in memory as stored, and every scan, which opens each cycle, writes back any
//! of them whose file was edited or deleted. A larger one stays as edited until the server next
//! changes it (the engine then stores a conflict rather than overwrite what it thinks is
//! someone's edit) or syncd next starts: [`PimMirror::open`] queues every item whose file is
//! damaged or missing to be fetched again.

use super::PimKind;
use super::vdir::{self, Meta};
use crate::dataset::{Dataset, DatasetError, DatasetId, fingerprint};
use crate::journal::{Journal, Op};
use porter_core::Bytes;
use porter_sync::{Blob, ConflictRule, ContentHash, ItemPath, ItemState, LocalId, Scanned};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The largest item kept in memory to be written back.
pub const ITEM_CAP: usize = 256 * 1024;
/// The most bytes of items kept in memory.
pub const TOTAL_CAP: usize = 32 * 1024 * 1024;

/// The items kept in memory as stored.
#[derive(Debug, Default)]
struct Pristine {
    bytes: usize,
    items: BTreeMap<LocalId, Vec<u8>>,
}

impl Pristine {
    fn drop_item(&mut self, local: &LocalId) {
        if let Some(old) = self.items.remove(local) {
            self.bytes -= old.len();
        }
    }

    /// Keeps `content` as `local`'s if it fits.
    fn keep(&mut self, local: &LocalId, content: &[u8]) {
        self.drop_item(local);
        if content.len() <= ITEM_CAP && self.bytes + content.len() <= TOTAL_CAP {
            self.bytes += content.len();
            self.items.insert(local.clone(), content.to_vec());
        }
    }
}

/// What the mirror stored for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    path: ItemPath,
    size: Bytes,
    hash: ContentHash,
}

#[derive(Debug)]
struct Inner {
    id: DatasetId,
    kind: PimKind,
    root: PathBuf,
    ledger: Mutex<BTreeMap<LocalId, Held>>,
    pristine: Mutex<Pristine>,
}

/// One collection's directory as a dataset. Cheap to clone: the clones are one mirror.
#[derive(Debug, Clone)]
pub struct PimMirror {
    inner: Arc<Inner>,
    healed: usize,
}

fn fail(what: &str, why: impl std::fmt::Display) -> DatasetError {
    DatasetError(format!("{what}: {why}"))
}

impl PimMirror {
    /// Opens the mirror of `root` (made when missing), named `id`, whose engine keeps `journal`.
    ///
    /// Before anything runs, every item the journal says is stored is checked against its file:
    /// one whose file is missing or whose bytes differ (an edit, a crash that left a short file,
    /// a conflict stored for it) loses its journal row, and the anchor is forgotten once, so the
    /// first cycle lists the collection again and fetches exactly those items.
    pub fn open(
        id: DatasetId,
        kind: PimKind,
        root: PathBuf,
        journal: &Journal,
    ) -> Result<Self, DatasetError> {
        std::fs::create_dir_all(&root).map_err(|e| fail("cannot make the collection", e))?;
        let rows = journal
            .items()
            .map_err(|e| fail("cannot read the journal", e))?;
        let conflicts = journal
            .conflicts()
            .map_err(|e| fail("cannot read the journal", e))?;
        let damaged: Vec<&_> = rows
            .iter()
            .filter(|row| match row.state {
                ItemState::Fetching | ItemState::Discarding => false,
                ItemState::Synced => {
                    let on_disk = std::fs::read(root.join(&row.local.0)).ok();
                    on_disk.map(|bytes| fingerprint(&bytes)) != row.hash
                }
                ItemState::Conflicted | ItemState::PendingUpload | ItemState::PendingRemove => true,
            })
            .collect();
        if !damaged.is_empty() {
            let mut ops: Vec<Op> = damaged
                .iter()
                .map(|row| Op::DeleteItem(row.local.clone()))
                .collect();
            ops.extend(
                conflicts
                    .iter()
                    .filter(|c| damaged.iter().any(|row| row.local == c.local))
                    .filter_map(|c| c.number.map(Op::DropConflict)),
            );
            ops.push(Op::ClearAnchor);
            journal
                .apply(&ops)
                .map_err(|e| fail("cannot repair the journal", e))?;
        }
        let ledger = journal
            .items()
            .map_err(|e| fail("cannot read the journal", e))?
            .into_iter()
            .filter_map(|row| {
                Some((
                    row.local,
                    Held {
                        path: row.path,
                        size: row.size,
                        hash: row.hash?,
                    },
                ))
            })
            .collect();
        Ok(Self {
            inner: Arc::new(Inner {
                id,
                kind,
                root,
                ledger: Mutex::new(ledger),
                pristine: Mutex::default(),
            }),
            healed: damaged.len(),
        })
    }

    /// How many items [`PimMirror::open`] found damaged and queued to be fetched again.
    pub fn healed(&self) -> usize {
        self.healed
    }

    /// The collection's directory.
    pub fn root(&self) -> &std::path::Path {
        &self.inner.root
    }

    /// Writes the collection's `displayname` and `color` files.
    pub fn write_meta(&self, meta: &Meta) -> Result<(), DatasetError> {
        vdir::write_meta(&self.inner.root, meta).map_err(|e| fail("cannot write the metadata", e))
    }
}

impl Inner {
    fn ledger(&self) -> MutexGuard<'_, BTreeMap<LocalId, Held>> {
        // Every critical section is a map update.
        self.ledger.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pristine(&self) -> MutexGuard<'_, Pristine> {
        self.pristine.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes back every kept item whose file is not what was stored.
    fn restore(&self) {
        let kept: Vec<(LocalId, Vec<u8>)> = self
            .pristine()
            .items
            .iter()
            .map(|(local, bytes)| (local.clone(), bytes.clone()))
            .collect();
        for (local, bytes) in kept {
            let held = std::fs::read(self.root.join(&local.0)).ok();
            if held.as_deref() != Some(bytes.as_slice()) {
                let _ = vdir::write_atomic(&self.root, &local.0, &bytes);
            }
        }
    }

    /// The file name for the item the server calls `path`, holding `content`: its UID, else the
    /// server's name, else that with a hash of the path; never one another item holds.
    fn name_for(&self, at: Option<&LocalId>, path: &ItemPath, content: &[u8]) -> String {
        let uid = vdir::uid_of(&String::from_utf8_lossy(content));
        let ledger = self.ledger();
        let free = |name: &str| {
            let held = ledger.get(&LocalId(name.to_owned()));
            held.is_none_or(|held| held.path == *path || at.is_some_and(|at| at.0 == name))
        };
        let preferred = vdir::file_name(self.kind, uid.as_deref(), &path.0);
        if free(&preferred) {
            return preferred;
        }
        let server = vdir::file_name(self.kind, None, &path.0);
        if free(&server) {
            return server;
        }
        let digest = Sha256::digest(path.0.as_bytes());
        let tag: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        let ext = self.kind.extension();
        let stem = server.strip_suffix(ext).unwrap_or(&server);
        format!("{stem}-{tag}{ext}")
    }

    fn checked(&self, item: &LocalId) -> Result<(), DatasetError> {
        match vdir::is_item_name(&item.0) {
            true => Ok(()),
            false => Err(DatasetError(format!(
                "{:?} is not a file of the mirror",
                item.0
            ))),
        }
    }

    fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        let name = self.name_for(at, path, &content.0);
        let hash = fingerprint(&content.0);
        let same = std::fs::read(self.root.join(&name)).is_ok_and(|held| held == content.0);
        if !same {
            vdir::write_atomic(&self.root, &name, &content.0)
                .map_err(|e| fail(&format!("cannot store {name}"), e))?;
        }
        let local = LocalId(name);
        let size = Bytes(content.0.len() as u64);
        self.pristine().keep(&local, &content.0);
        let mut ledger = self.ledger();
        if let Some(old) = at.filter(|old| **old != local) {
            // The item moved to another file name; the old file goes once the new one is whole.
            vdir::remove_file(&self.root, &old.0)
                .map_err(|e| fail(&format!("cannot remove {}", old.0), e))?;
            ledger.remove(old);
            self.pristine().drop_item(old);
        }
        ledger.insert(
            local.clone(),
            Held {
                path: path.clone(),
                size,
                hash: hash.clone(),
            },
        );
        Ok(Scanned {
            local,
            path: path.clone(),
            size,
            hash,
        })
    }

    fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        self.checked(item)?;
        vdir::remove_file(&self.root, &item.0)
            .map_err(|e| fail(&format!("cannot remove {}", item.0), e))?;
        self.ledger().remove(item);
        self.pristine().drop_item(item);
        Ok(())
    }

    fn scan(&self) -> Vec<Scanned> {
        self.restore();
        self.ledger()
            .iter()
            .map(|(local, held)| Scanned {
                local: local.clone(),
                path: held.path.clone(),
                size: held.size,
                hash: held.hash.clone(),
            })
            .collect()
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, DatasetError> + Send + 'static,
) -> Result<T, DatasetError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| fail("the file task stopped", e))?
}

impl Dataset for PimMirror {
    fn id(&self) -> DatasetId {
        self.inner.id.clone()
    }

    fn conflict_rule(&self) -> ConflictRule {
        // The server's copy always wins; nothing here is ever uploaded, so none can arise.
        ConflictRule::ShowInApp
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        let inner = Arc::clone(&self.inner);
        blocking(move || Ok(inner.scan())).await
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        self.inner.checked(item)?;
        let (inner, item) = (Arc::clone(&self.inner), item.clone());
        blocking(move || {
            std::fs::read(inner.root.join(&item.0))
                .map(Blob)
                .map_err(|e| fail(&format!("cannot read {}", item.0), e))
        })
        .await
    }

    async fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        let (inner, at, path) = (Arc::clone(&self.inner), at.cloned(), path.clone());
        blocking(move || inner.store(at.as_ref(), &path, content)).await
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        let (inner, item) = (Arc::clone(&self.inner), item.clone());
        blocking(move || inner.discard(&item)).await
    }
}

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod tests;
