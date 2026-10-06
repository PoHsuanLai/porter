//! The mirror under the real engine over a `MemoryReplica`: nothing local is ever uploaded or
//! removed on the replica, and a restart fetches again what is damaged.

use super::tests::ics;
use super::*;
use crate::clock::ManualClock;
use crate::datasets::pim::items_in;
use crate::engine::Engine;
use crate::testing::scratch;
use porter_core::UnixSeconds;
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_sync::RemoteId;
use porter_sync::{
    BaseVersion, ByteRange, ChangePage, Cursor, MemoryReplica, PutItem, PutRefused, PutTarget,
    Quota, RemoteVersion, Replica, ReplicaError,
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A replica shared between engines (a restart), counting what is asked of it.
#[derive(Debug, Clone)]
struct Server(Arc<Counted>);

#[derive(Debug)]
struct Counted {
    inner: MemoryReplica,
    fetches: AtomicUsize,
    writes: AtomicUsize,
}

impl Replica for Server {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
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
        self.0.writes.fetch_add(1, Ordering::Relaxed);
        self.0.inner.put(item, base).await
    }

    async fn remove(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        self.0.writes.fetch_add(1, Ordering::Relaxed);
        self.0.inner.remove(item, base).await
    }

    async fn quota(&self) -> Result<Quota, ReplicaError> {
        self.0.inner.quota().await
    }

    fn features(&self) -> StorageCap {
        self.0.inner.features()
    }
}

impl Server {
    fn new() -> Self {
        let features = StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Unreported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::None,
            ranges: Offered::Present,
            chunked_upload: Offered::Absent,
        };
        Self(Arc::new(Counted {
            inner: MemoryReplica::new(features, 10, UnixSeconds(1_000)),
            fetches: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        }))
    }

    /// The server's own doing: a new item, or the item at `path` replaced. Not counted.
    async fn set(&self, path: &str, content: Blob) {
        let listed = self.0.inner.changes(Cursor::Start).await.expect("list");
        let held = listed.changes.iter().find_map(|change| match change {
            porter_sync::Change::Upsert(item) if item.path.0 == path => Some(item.clone()),
            _ => None,
        });
        let (target, base) = match held {
            Some(item) => (PutTarget::Existing(item.id), BaseVersion::At(item.version)),
            None => (
                PutTarget::New(ItemPath(path.to_owned())),
                BaseVersion::Absent,
            ),
        };
        let item = PutItem {
            target,
            content,
            hash: None,
        };
        self.0.inner.put(item, base).await.expect("server write");
    }

    fn fetches(&self) -> usize {
        self.0.fetches.load(Ordering::Relaxed)
    }

    fn writes(&self) -> usize {
        self.0.writes.load(Ordering::Relaxed)
    }
}

fn big(uid: &str, tag: &str) -> Blob {
    let pad = "X".repeat(300 * 1024);
    Blob(
        format!("BEGIN:VCALENDAR\r\nUID:{uid}\r\nDESCRIPTION:{tag}{pad}\r\nEND:VCALENDAR\r\n")
            .into_bytes(),
    )
}

fn engine(
    server: &Server,
    dir: &std::path::Path,
) -> (Engine<Server, PimMirror, Arc<ManualClock>>, PimMirror) {
    let journal = Journal::open(&dir.join("j.sqlite")).expect("journal");
    let mirror = PimMirror::open(
        DatasetId::parse("pim_cal_personal").expect("id"),
        PimKind::Calendar,
        dir.join("vdir"),
        &journal,
    )
    .expect("open");
    let clock = Arc::new(ManualClock::at(5_000));
    (
        Engine::new(server.clone(), mirror.clone(), journal, clock),
        mirror,
    )
}

#[tokio::test]
async fn a_local_edit_a_deletion_and_a_stray_file_cost_the_server_nothing_and_the_next_change_overwrites()
 {
    let dir = scratch("engine-readonly");
    let server = Server::new();
    server.set("a.ics", ics("a", "one")).await;
    server.set("big.ics", big("big", "v1")).await;
    let (engine, mirror) = engine(&server, &dir);
    let first = engine.sync_once().await.expect("sync");
    assert_eq!((first.fetched, first.uploaded, first.removed), (2, 0, 0));
    assert_eq!(
        items_in(mirror.root(), PimKind::Calendar),
        ["a.ics", "big.ics"]
    );

    std::fs::write(mirror.root().join("big.ics"), "edited").expect("edit");
    std::fs::write(mirror.root().join("stray.ics"), "mine").expect("stray");
    std::fs::remove_file(mirror.root().join("a.ics")).expect("delete");
    let again = engine.sync_once().await.expect("sync");
    assert_eq!(
        (
            again.fetched,
            again.uploaded,
            again.removed,
            again.conflicts.len()
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(server.writes(), 0, "nothing was uploaded or removed");
    assert!(
        !mirror.root().join("a.ics").exists(),
        "no server change, no write-back"
    );

    // The server changes both: the server's state wins, with no conflict.
    server.set("a.ics", ics("a", "two")).await;
    server.set("big.ics", big("big", "v2")).await;
    let changed = engine.sync_once().await.expect("sync");
    assert_eq!((changed.fetched, changed.conflicts.len()), (2, 0));
    assert_eq!(
        std::fs::read(mirror.root().join("a.ics")).expect("a"),
        ics("a", "two").0
    );
    assert_eq!(
        std::fs::read(mirror.root().join("big.ics")).expect("big"),
        big("big", "v2").0
    );
    assert_eq!(server.writes(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_restart_fetches_again_exactly_the_items_whose_files_were_damaged() {
    let dir = scratch("engine-restart");
    let server = Server::new();
    server.set("a.ics", ics("a", "one")).await;
    server.set("big.ics", big("big", "v1")).await;
    server.set("fine.ics", ics("fine", "ok")).await;
    let (first, mirror) = engine(&server, &dir);
    first.sync_once().await.expect("sync");
    let fetched = server.fetches();
    assert_eq!(fetched, 3);

    // Damaged while syncd ran and while it was down: a deleted file is no server change, so only
    // a restart brings it back.
    std::fs::write(mirror.root().join("big.ics"), "edited").expect("edit");
    std::fs::remove_file(mirror.root().join("a.ics")).expect("delete");
    drop(first);
    let (second, reopened) = engine(&server, &dir);
    assert_eq!(reopened.healed(), 2);
    let report = second.sync_once().await.expect("sync");
    assert_eq!((report.fetched, report.uploaded, report.removed), (2, 0, 0));
    assert!(report.conflicts.is_empty());
    assert_eq!(
        server.fetches(),
        fetched + 2,
        "those two items and no other"
    );
    assert_eq!(server.writes(), 0);
    assert_eq!(
        std::fs::read(reopened.root().join("big.ics")).expect("big"),
        big("big", "v1").0
    );
    assert_eq!(
        std::fs::read(reopened.root().join("a.ics")).expect("a"),
        ics("a", "one").0
    );
    let _ = std::fs::remove_dir_all(dir);
}
