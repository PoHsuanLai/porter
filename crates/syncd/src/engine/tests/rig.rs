//! The shared test rig: a world of one replica, one dataset, one clock and a journal file that
//! engines can be opened on again, as a restarted process would.

use crate::clock::ManualClock;
use crate::dataset::MemoryDataset;
use crate::engine::{Engine, Report};
use crate::journal::Journal;
use crate::testing::scratch;
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{Bytes, UnixSeconds};
use porter_sync::{
    BaseVersion, Blob, ByteRange, Change, ChangePage, ConflictRule, Cursor, ItemPath,
    MemoryReplica, PutItem, PutRefused, PutTarget, Quota, RemoteId, RemoteVersion, Replica,
    ReplicaError,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

pub fn caps(hashes: HashKind, quota: QuotaReport, delta: Delta) -> StorageCap {
    StorageCap {
        access: Access::ReadWrite,
        delta,
        quota,
        scope: StorageScope::AppFolder,
        hashes,
        ranges: Offered::Present,
        chunked_upload: Offered::Absent,
    }
}

/// A `MemoryReplica` that can fail once on request and counts what it was asked.
#[derive(Debug)]
pub struct Faulty {
    pub inner: MemoryReplica,
    pub changes_fault: Mutex<Option<ReplicaError>>,
    pub put_fault: Mutex<Option<PutRefused>>,
    pub puts: AtomicUsize,
    pub fetches: AtomicUsize,
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

impl Faulty {
    pub fn fail_next_changes(&self, error: ReplicaError) {
        *self
            .changes_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    pub fn fail_next_put(&self, refused: PutRefused) {
        *self
            .put_fault
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(refused);
    }
}

/// The replica engines share (the daemon will share one between an engine and its quota reads).
#[derive(Debug, Clone)]
pub struct Shared(pub Arc<Faulty>);

impl Replica for Shared {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        if let Some(fault) = take(&self.0.changes_fault) {
            return Err(fault);
        }
        self.0.inner.changes(from).await
    }

    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        self.0.fetches.fetch_add(1, Ordering::Relaxed);
        self.0.inner.fetch(item, range).await
    }

    async fn put(
        &self,
        item: PutItem,
        base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        self.0.puts.fetch_add(1, Ordering::Relaxed);
        if let Some(fault) = take(&self.0.put_fault) {
            return Err(fault);
        }
        self.0.inner.put(item, base).await
    }

    async fn remove(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        self.0.inner.remove(item, base).await
    }

    async fn quota(&self) -> Result<Quota, ReplicaError> {
        self.0.inner.quota().await
    }

    fn features(&self) -> StorageCap {
        self.0.inner.features()
    }
}

pub type TestEngine = Engine<Shared, Arc<MemoryDataset>, Arc<ManualClock>>;

/// One replica, one dataset, one clock and a journal path.
pub struct World {
    pub replica: Arc<Faulty>,
    pub dataset: Arc<MemoryDataset>,
    pub clock: Arc<ManualClock>,
    pub dir: PathBuf,
}

impl World {
    pub fn new(name: &str, features: StorageCap, page: usize) -> Self {
        Self::limited(name, features, page, None)
    }

    pub fn limited(name: &str, features: StorageCap, page: usize, limit: Option<Bytes>) -> Self {
        let mut inner = MemoryReplica::new(features, page, UnixSeconds(1_000));
        if let Some(limit) = limit {
            inner = inner.with_limit(limit);
        }
        Self {
            replica: Arc::new(Faulty {
                inner,
                changes_fault: Mutex::default(),
                put_fault: Mutex::default(),
                puts: AtomicUsize::new(0),
                fetches: AtomicUsize::new(0),
            }),
            dataset: Arc::new(MemoryDataset::new("files", ConflictRule::KeepBoth)),
            clock: Arc::new(ManualClock::at(5_000)),
            dir: scratch(name),
        }
    }

    /// A fresh engine on the journal file: what a restarted process has.
    pub fn engine(&self) -> TestEngine {
        let journal = Journal::open(&self.dir.join("files.sqlite")).expect("journal");
        Engine::new(
            Shared(Arc::clone(&self.replica)),
            Arc::clone(&self.dataset),
            journal,
            Arc::clone(&self.clock),
        )
    }

    /// Someone else writes a file to the replica.
    pub async fn remote_put(&self, path: &str, bytes: &[u8]) -> RemoteId {
        let item = PutItem {
            target: PutTarget::New(ItemPath(path.into())),
            content: Blob(bytes.to_vec()),
            hash: crate::dataset::replica_hash(self.replica.inner.features().hashes, bytes),
        };
        self.replica
            .inner
            .put(item, BaseVersion::Absent)
            .await
            .expect("remote put")
            .0
    }

    /// Someone else changes a file on the replica (at its current version).
    pub async fn remote_edit(&self, id: &RemoteId, bytes: &[u8]) {
        let current = self.version_of(id).await;
        let item = PutItem {
            target: PutTarget::Existing(id.clone()),
            content: Blob(bytes.to_vec()),
            hash: crate::dataset::replica_hash(self.replica.inner.features().hashes, bytes),
        };
        self.replica
            .inner
            .put(item, BaseVersion::At(current))
            .await
            .expect("remote edit");
    }

    /// Someone else deletes a file on the replica.
    pub async fn remote_remove(&self, id: &RemoteId) {
        let current = self.version_of(id).await;
        self.replica
            .inner
            .remove(id, BaseVersion::At(current))
            .await
            .expect("remote remove");
    }

    async fn version_of(&self, id: &RemoteId) -> RemoteVersion {
        let page = self
            .replica
            .inner
            .changes(Cursor::Start)
            .await
            .expect("listing");
        page.changes
            .into_iter()
            .find_map(|change| match change {
                Change::Upsert(item) if item.id == *id => Some(item.version),
                _ => None,
            })
            .expect("the item is live")
    }

    /// The replica's live files by path.
    pub async fn remote_files(&self) -> BTreeMap<String, Vec<u8>> {
        let page = self
            .replica
            .inner
            .changes(Cursor::Start)
            .await
            .expect("listing");
        let mut files = BTreeMap::new();
        for change in page.changes {
            if let Change::Upsert(item) = change {
                let blob = self
                    .replica
                    .inner
                    .fetch(&item.id, ByteRange::Whole)
                    .await
                    .expect("fetch");
                files.insert(item.path.0, blob.0);
            }
        }
        files
    }

    pub fn puts(&self) -> usize {
        self.replica.puts.load(Ordering::Relaxed)
    }

    pub fn fetches(&self) -> usize {
        self.replica.fetches.load(Ordering::Relaxed)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn sha() -> StorageCap {
    caps(HashKind::Sha256, QuotaReport::Reported, Delta::Poll)
}

pub fn hashless() -> StorageCap {
    caps(HashKind::None, QuotaReport::Unreported, Delta::Poll)
}

/// Syncs until a cycle does nothing, at most `limit` cycles; the last report.
pub async fn settle(engine: &TestEngine, limit: usize) -> Report {
    let mut last = Report::default();
    for _ in 0..limit {
        last = engine.sync_once().await.expect("cycle");
        let did =
            last.fetched + last.uploaded + last.removed + last.discarded + last.conflicts.len();
        if did == 0 {
            break;
        }
    }
    last
}
