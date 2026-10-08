use super::rig::*;
use crate::dataset::{Dataset, DatasetError, DatasetId, MemoryDataset};
use crate::engine::{Engine, Outcome};
use crate::journal::Journal;
use porter_core::Bytes;
use porter_core::capability::{Delta, HashKind, QuotaReport};
use porter_sync::{
    Acknowledgement, BaseVersion, Blob, ConflictRule, ItemPath, ItemState, LocalId, PutRefused,
    RemoteId, RemoteSide, ReplicaError, Resolution, RetryAfter, Scanned,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
        .collect()
}

fn states(engine: &TestEngine) -> Vec<(String, ItemState)> {
    engine
        .journal()
        .items()
        .expect("items")
        .into_iter()
        .map(|row| (row.path.0, row.state))
        .collect()
}

#[tokio::test]
async fn a_first_sync_lists_everything_and_a_later_feed_pages_until_done() {
    let world = World::new("first", sha(), 2);
    for n in 0..5 {
        world
            .remote_put(&format!("f{n}.txt"), format!("v{n}").as_bytes())
            .await;
    }
    let engine = world.engine();
    let first = engine.sync_once().await.expect("cycle");
    assert_eq!((first.fetched, first.outcome), (5, Outcome::Done));
    assert_eq!(world.dataset.snapshot(), world.remote_files().await);
    assert!(engine.journal().anchor().expect("anchor").is_some());

    for n in 5..10 {
        world
            .remote_put(&format!("f{n}.txt"), format!("v{n}").as_bytes())
            .await;
    }
    let second = engine.sync_once().await.expect("cycle");
    assert_eq!(second.fetched, 5, "three pages of two, then done");
    assert_eq!(world.dataset.snapshot().len(), 10);
    let idle = engine.sync_once().await.expect("cycle");
    assert_eq!((idle.fetched, idle.uploaded), (0, 0));
    assert_eq!(world.puts(), 0, "nothing was uploaded");
    assert_eq!(world.fetches(), 10, "and nothing fetched twice");
    assert!(states(&engine).iter().all(|(_, s)| *s == ItemState::Synced));
}

#[tokio::test]
async fn a_new_local_file_uploads_once_on_an_absent_base_and_its_echo_is_ignored() {
    let world = World::new("upload", sha(), 10);
    let engine = world.engine();
    world.dataset.put("a.txt", b"one");
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.uploaded, 1);
    assert_eq!(world.remote_files().await, files(&[("a.txt", "one")]));
    let rows = engine.journal().items().expect("items");
    let row = &rows[0];
    assert_eq!(row.state, ItemState::Synced);
    assert!(row.remote.is_some());
    assert_eq!(
        row.base,
        BaseVersion::At(row.remote_version.clone().expect("version"))
    );
    let again = engine.sync_once().await.expect("cycle");
    assert_eq!((again.uploaded, again.fetched), (0, 0));
    assert_eq!((world.puts(), world.fetches()), (1, 0));
}

#[tokio::test]
async fn a_local_edit_uploads_over_the_version_it_was_based_on() {
    let world = World::new("edit", sha(), 10);
    let engine = world.engine();
    world.dataset.put("a.txt", b"one");
    settle(&engine, 4).await;
    let before = engine.journal().items().expect("items")[0]
        .remote_version
        .clone();
    world.dataset.put("a.txt", b"two");
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.uploaded, 1);
    assert_eq!(world.remote_files().await, files(&[("a.txt", "two")]));
    let after = engine.journal().items().expect("items")[0]
        .remote_version
        .clone();
    assert_ne!(before, after);
    settle(&engine, 4).await;
    assert_eq!(world.puts(), 2);
}

#[tokio::test]
async fn crossing_edits_are_a_stored_conflict_and_neither_side_is_overwritten() {
    let world = World::new("cross", sha(), 10);
    let id = world.remote_put("a.txt", b"one").await;
    let engine = world.engine();
    settle(&engine, 4).await;
    world.remote_edit(&id, b"theirs").await;
    world.dataset.put("a.txt", b"mine");
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.conflicts.len(), 1);
    let conflict = &report.conflicts[0];
    assert_eq!(conflict.conflict.item, id);
    assert!(matches!(conflict.conflict.base, BaseVersion::At(_)));
    assert!(matches!(conflict.conflict.remote, RemoteSide::Changed(_)));
    assert_eq!(world.dataset.get("a.txt"), Some(b"mine".to_vec()));
    assert_eq!(world.remote_files().await, files(&[("a.txt", "theirs")]));
    assert_eq!(
        states(&engine),
        [("a.txt".to_owned(), ItemState::Conflicted)]
    );
    assert_eq!(engine.journal().conflicts().expect("conflicts").len(), 1);
    // It stays: a later cycle neither piles up conflicts nor touches either side.
    let later = settle(&engine, 3).await;
    assert!(later.conflicts.is_empty());
    assert_eq!(engine.journal().conflicts().expect("conflicts").len(), 1);
    assert_eq!(world.dataset.get("a.txt"), Some(b"mine".to_vec()));
}

#[tokio::test]
async fn a_conflict_settles_either_way_and_the_sides_converge() {
    for (how, winner) in [
        (Resolution::KeepLocal, "mine"),
        (Resolution::KeepRemote, "theirs"),
    ] {
        let world = World::new("settle", sha(), 10);
        let id = world.remote_put("a.txt", b"one").await;
        let engine = world.engine();
        settle(&engine, 4).await;
        world.remote_edit(&id, b"theirs").await;
        world.dataset.put("a.txt", b"mine");
        engine.sync_once().await.expect("cycle");
        let number = engine.journal().conflicts().expect("conflicts")[0]
            .number
            .expect("number");
        assert!(engine.resolve(number, how).expect("resolve"));
        assert!(
            !engine.resolve(number, how).expect("again"),
            "already settled"
        );
        settle(&engine, 5).await;
        assert_eq!(
            world.remote_files().await,
            files(&[("a.txt", winner)]),
            "{how:?}"
        );
        assert_eq!(
            world.dataset.snapshot(),
            files(&[("a.txt", winner)]),
            "{how:?}"
        );
        assert!(engine.journal().conflicts().expect("conflicts").is_empty());
        assert!(states(&engine).iter().all(|(_, s)| *s == ItemState::Synced));
    }
}

#[tokio::test]
async fn an_edit_against_a_remote_delete_and_a_delete_against_a_remote_edit_are_conflicts() {
    let world = World::new("deletes", sha(), 10);
    let a = world.remote_put("a.txt", b"one").await;
    let b = world.remote_put("b.txt", b"one").await;
    let engine = world.engine();
    settle(&engine, 4).await;
    world.remote_remove(&a).await;
    world.dataset.put("a.txt", b"edited");
    world.remote_edit(&b, b"theirs").await;
    world.dataset.remove("b.txt");
    let report = engine.sync_once().await.expect("cycle");
    let mut sides: Vec<(String, bool)> = report
        .conflicts
        .iter()
        .map(|c| {
            (
                c.conflict.item.0.clone(),
                matches!(c.conflict.remote, RemoteSide::Deleted(_)),
            )
        })
        .collect();
    sides.sort();
    let mut want = vec![(a.0, true), (b.0, false)];
    want.sort();
    assert_eq!(sides, want);
    assert_eq!(
        world.dataset.get("a.txt"),
        Some(b"edited".to_vec()),
        "the local edit survives"
    );
    assert_eq!(
        world.remote_files().await,
        files(&[("b.txt", "theirs")]),
        "the remote edit survives"
    );
}

#[tokio::test]
async fn remote_changes_and_deletions_arrive_and_a_deletion_is_acknowledged_then_compacted() {
    let world = World::new("remote", sha(), 10);
    let a = world.remote_put("a.txt", b"one").await;
    let b = world.remote_put("b.txt", b"one").await;
    let engine = world.engine();
    settle(&engine, 4).await;
    world.remote_edit(&a, b"two").await;
    world.remote_remove(&b).await;
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!((report.fetched, report.discarded), (1, 1));
    assert_eq!(world.dataset.snapshot(), files(&[("a.txt", "two")]));
    assert!(
        engine
            .journal()
            .tombstones()
            .expect("tombstones")
            .is_empty(),
        "applied, acknowledged, compacted"
    );
}

#[tokio::test]
async fn a_local_delete_removes_remotely_keeps_a_tombstone_until_the_feed_shows_it_then_compacts() {
    let world = World::new("localdel", sha(), 10);
    world.remote_put("a.txt", b"one").await;
    let engine = world.engine();
    settle(&engine, 4).await;
    world.dataset.remove("a.txt");
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.removed, 1);
    assert!(world.remote_files().await.is_empty());
    let kept = engine.journal().tombstones().expect("tombstones");
    assert_eq!(kept.len(), 1, "kept until acknowledged");
    assert_eq!(kept[0].ack, Acknowledgement::Pending);
    assert!(engine.journal().items().expect("items").is_empty());
    engine.sync_once().await.expect("cycle");
    assert!(
        engine
            .journal()
            .tombstones()
            .expect("tombstones")
            .is_empty(),
        "the feed showed it: compacted"
    );
}

#[tokio::test]
async fn an_expired_anchor_is_a_full_listing_reconciled_by_content_with_no_reupload() {
    let world = World::new("expired", sha(), 10);
    let keep = world.remote_put("keep.txt", b"same").await;
    let gone = world.remote_put("gone.txt", b"bye").await;
    let engine = world.engine();
    world.dataset.put("mine.txt", b"local only");
    settle(&engine, 4).await;
    let (puts, fetches) = (world.puts(), world.fetches());
    // The server forgets its history, and meanwhile: one item vanishes, one is rewritten with
    // the same bytes (a new version), one is new.
    world.remote_remove(&gone).await;
    world.remote_remove(&keep).await;
    world.remote_put("keep.txt", b"same").await;
    world.remote_put("new.txt", b"fresh").await;
    world.replica.inner.compact();
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.outcome, Outcome::Done);
    assert_eq!(world.puts(), puts, "nothing known was uploaded again");
    assert_eq!(
        world.fetches(),
        fetches + 1,
        "only new.txt came down; keep.txt matched by hash"
    );
    assert_eq!(
        world.dataset.snapshot(),
        files(&[
            ("keep.txt", "same"),
            ("mine.txt", "local only"),
            ("new.txt", "fresh")
        ])
    );
    assert!(
        engine.journal().anchor().expect("anchor").is_some(),
        "the listing's anchor is stored"
    );
    let later = settle(&engine, 3).await;
    assert_eq!((later.fetched, later.uploaded), (0, 0));
}

/// Three synced files, then the server forgets its history and lists nothing.
async fn after_an_empty_listing(name: &str) -> (World, TestEngine, Vec<RemoteId>) {
    let world = World::new(name, sha(), 10);
    let mut ids = Vec::new();
    for file in ["a.txt", "b.txt", "c.txt"] {
        ids.push(world.remote_put(file, file.as_bytes()).await);
    }
    let engine = world.engine();
    settle(&engine, 4).await;
    assert_eq!(world.dataset.snapshot().len(), 3);
    for id in &ids {
        world.remote_remove(id).await;
    }
    world.replica.inner.compact();
    (world, engine, ids)
}

#[tokio::test]
async fn an_empty_listing_after_a_full_one_discards_nothing_until_the_person_confirms() {
    let (world, engine, _) = after_an_empty_listing("mass").await;
    let held = porter_sync::MassDelete {
        discard: 3,
        held: 3,
    };
    for _ in 0..2 {
        let report = engine.sync_once().await.expect("cycle");
        assert_eq!(report.outcome, Outcome::NeedsConfirmation(held));
        assert_eq!(report.discarded, 0);
        assert_eq!(world.dataset.snapshot().len(), 3, "the local files are kept");
        assert!(
            states(&engine).iter().all(|(_, s)| *s == ItemState::Synced),
            "the journal is untouched"
        );
        assert!(
            engine.journal().anchor().expect("anchor").is_none(),
            "so the listing is asked for again next cycle"
        );
    }
    engine.confirm_mass_delete();
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!((report.outcome, report.discarded), (Outcome::Done, 3));
    assert!(world.dataset.snapshot().is_empty());
    // The confirmation was for that listing only.
    assert_eq!(settle(&engine, 3).await.outcome, Outcome::Done);
}

#[tokio::test]
async fn a_server_that_lists_its_items_again_is_not_held_and_loses_nothing() {
    let (world, engine, _) = after_an_empty_listing("mass-recovers").await;
    assert!(matches!(
        engine.sync_once().await.expect("cycle").outcome,
        Outcome::NeedsConfirmation(_)
    ));
    // The server was in trouble, not emptied: the same files are there again (new ids).
    let fetches = world.fetches();
    for file in ["a.txt", "b.txt", "c.txt"] {
        world.remote_put(file, file.as_bytes()).await;
    }
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.outcome, Outcome::Done);
    assert_eq!((report.discarded, report.fetched), (0, 0));
    assert_eq!(world.fetches(), fetches, "nothing came down again");
    assert_eq!(world.dataset.snapshot().len(), 3);
}

#[tokio::test]
async fn a_replica_without_hashes_is_reconciled_by_bytes_and_still_uploads_nothing_known() {
    let world = World::new("hashless", hashless(), 10);
    let a = world.remote_put("a.txt", b"same").await;
    let engine = world.engine();
    settle(&engine, 4).await;
    world.remote_remove(&a).await;
    world.remote_put("a.txt", b"same").await;
    world.replica.inner.compact();
    let before = world.puts();
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!((report.uploaded, report.conflicts.len()), (0, 0));
    assert_eq!(world.puts(), before);
    assert_eq!(world.dataset.snapshot(), files(&[("a.txt", "same")]));
}

#[tokio::test]
async fn the_same_new_path_with_the_same_bytes_is_adopted_and_with_other_bytes_a_conflict() {
    let world = World::new("exists", sha(), 10);
    world.remote_put("same.txt", b"x").await;
    world.remote_put("diff.txt", b"theirs").await;
    world.dataset.put("same.txt", b"x");
    world.dataset.put("diff.txt", b"mine");
    let engine = world.engine();
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.uploaded, 0, "nothing is written over what is there");
    assert_eq!(report.conflicts.len(), 1);
    assert!(matches!(
        report.conflicts[0].conflict.remote,
        RemoteSide::Exists(..)
    ));
    assert_eq!(report.conflicts[0].conflict.base, BaseVersion::Absent);
    assert_eq!(world.dataset.get("diff.txt"), Some(b"mine".to_vec()));
    assert_eq!(world.remote_files().await["diff.txt"], b"theirs");
    let by_path: BTreeMap<_, _> = states(&engine).into_iter().collect();
    assert_eq!(by_path["same.txt"], ItemState::Synced);
    assert_eq!(by_path["diff.txt"], ItemState::Conflicted);
}

#[tokio::test]
async fn a_transient_or_unauthorized_replica_stops_the_cycle_and_changes_nothing() {
    let world = World::new("faults", sha(), 10);
    world.remote_put("a.txt", b"one").await;
    let engine = world.engine();
    world
        .replica
        .fail_next_changes(ReplicaError::Transient(RetryAfter(120)));
    let stopped = engine.sync_once().await.expect("a stop is not an error");
    assert_eq!(stopped.outcome, Outcome::Retry(RetryAfter(120)));
    assert!(world.dataset.snapshot().is_empty());
    assert!(engine.journal().anchor().expect("anchor").is_none());
    world.replica.fail_next_changes(ReplicaError::Unauthorized);
    assert_eq!(
        engine.sync_once().await.expect("cycle").outcome,
        Outcome::Unauthorized
    );
    world.replica.fail_next_changes(ReplicaError::Gone);
    assert_eq!(
        engine.sync_once().await.expect("cycle").outcome,
        Outcome::Gone
    );
    let ok = engine.sync_once().await.expect("cycle");
    assert_eq!((ok.outcome, ok.fetched), (Outcome::Done, 1));

    world.dataset.put("b.txt", b"two");
    world
        .replica
        .fail_next_put(PutRefused::Transient(RetryAfter(30)));
    let retry = engine.sync_once().await.expect("cycle");
    assert_eq!(retry.outcome, Outcome::Retry(RetryAfter(30)));
    assert_eq!(
        states(&engine)[1],
        ("b.txt".to_owned(), ItemState::PendingUpload)
    );
    let done = engine.sync_once().await.expect("cycle");
    assert_eq!((done.outcome, done.uploaded), (Outcome::Done, 1));
}

#[tokio::test]
async fn a_full_replica_holds_uploads_back_but_removals_and_pulls_go_on_and_quota_is_reported() {
    let world = World::limited(
        "quota",
        caps(HashKind::Sha256, QuotaReport::Reported, Delta::Poll),
        10,
        Some(Bytes(10)),
    );
    let old = world.remote_put("old.txt", b"123456").await;
    let engine = world.engine();
    settle(&engine, 4).await;
    world.dataset.put("big.txt", b"too big for the rest");
    world.dataset.remove("old.txt");
    world.remote_put("pulled.txt", b"abc").await;
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.outcome, Outcome::QuotaFull);
    assert_eq!(report.removed, 1, "the removal still went");
    assert_eq!(report.fetched, 1, "and the pull");
    assert_eq!(report.quota.map(|q| q.total), Some(Some(Bytes(10))));
    assert!(world.dataset.get("pulled.txt").is_some());
    assert_eq!(
        engine
            .journal()
            .items()
            .expect("items")
            .iter()
            .filter(|r| r.state == ItemState::PendingUpload)
            .count(),
        1
    );
    let _ = old;
}

#[tokio::test]
async fn a_forbidden_write_stays_pending_and_is_counted() {
    let world = World::new("forbidden", sha(), 10);
    let engine = world.engine();
    world.dataset.put("a.txt", b"one");
    world.replica.fail_next_put(PutRefused::Forbidden);
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!((report.refused, report.uploaded), (1, 0));
    assert_eq!(
        states(&engine),
        [("a.txt".to_owned(), ItemState::PendingUpload)]
    );
    assert_eq!(engine.sync_once().await.expect("cycle").uploaded, 1);
}

/// A dataset whose file is edited by the user right after the engine's scan: the race the
/// engine's pre-store check exists for.
struct Racy {
    inner: Arc<MemoryDataset>,
    edit: Mutex<Option<(String, Vec<u8>)>>,
}

impl Dataset for Racy {
    fn id(&self) -> DatasetId {
        self.inner.id()
    }

    fn conflict_rule(&self) -> ConflictRule {
        self.inner.conflict_rule()
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        let seen = self.inner.scan().await;
        let edit = self
            .edit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some((path, bytes)) = edit {
            self.inner.put(&path, &bytes);
        }
        seen
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        self.inner.read(item).await
    }

    async fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        self.inner.store(at, path, content).await
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        self.inner.discard(item).await
    }
}

#[tokio::test]
async fn a_local_edit_made_after_the_scan_is_never_overwritten_by_a_fetch() {
    let world = World::new("racy", sha(), 10);
    let id = world.remote_put("a.txt", b"one").await;
    settle(&world.engine(), 4).await;
    let racy = Racy {
        inner: Arc::clone(&world.dataset),
        edit: Mutex::new(Some(("a.txt".to_owned(), b"mine".to_vec()))),
    };
    let engine = Engine::new(
        Shared(Arc::clone(&world.replica)),
        racy,
        Journal::open(&world.dir.join("files.sqlite")).expect("journal"),
        Arc::clone(&world.clock),
    );
    world.remote_edit(&id, b"theirs").await;
    let report = engine.sync_once().await.expect("cycle");
    assert_eq!(report.conflicts.len(), 1, "the edit surfaced as a conflict");
    assert_eq!(
        world.dataset.get("a.txt"),
        Some(b"mine".to_vec()),
        "and was not overwritten"
    );
    assert_eq!(world.remote_files().await, files(&[("a.txt", "theirs")]));
}

/// A dataset that only mirrors: the memory dataset, told to travel replica to local.
#[derive(Debug)]
struct Mirror(Arc<MemoryDataset>);

impl Dataset for Mirror {
    fn id(&self) -> DatasetId {
        self.0.id()
    }

    fn conflict_rule(&self) -> ConflictRule {
        self.0.conflict_rule()
    }

    fn direction(&self) -> crate::dataset::Direction {
        crate::dataset::Direction::PullOnly
    }

    async fn scan(&self) -> Result<Vec<Scanned>, DatasetError> {
        self.0.scan().await
    }

    async fn read(&self, item: &LocalId) -> Result<Blob, DatasetError> {
        self.0.read(item).await
    }

    async fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> Result<Scanned, DatasetError> {
        self.0.store(at, path, content).await
    }

    async fn discard(&self, item: &LocalId) -> Result<(), DatasetError> {
        self.0.discard(item).await
    }
}

/// What a locally edited item, a locally deleted one and a new local file become, by direction:
/// two-way uploads, removes and conflicts; pull-only does none of it and the server's next
/// change overwrites the edit.
#[tokio::test]
async fn a_local_edit_is_a_conflict_or_an_upload_two_way_and_overwritten_pull_only() {
    for pull_only in [false, true] {
        let world = World::new(if pull_only { "pull" } else { "two" }, sha(), 10);
        let edited = world.remote_put("edited.txt", b"v1").await;
        world.remote_put("deleted.txt", b"v1").await;
        let journal = Journal::open(&world.dir.join("files.sqlite")).expect("journal");
        let clock = Arc::clone(&world.clock);
        let replica = Shared(Arc::clone(&world.replica));
        let report = if pull_only {
            let engine = Engine::new(replica, Mirror(Arc::clone(&world.dataset)), journal, clock);
            engine.sync_once().await.expect("first");
            world.dataset.put("edited.txt", b"mine");
            world.dataset.remove("deleted.txt");
            world.dataset.put("new.txt", b"mine");
            world.remote_edit(&edited, b"v2").await;
            engine.sync_once().await.expect("second")
        } else {
            let engine = Engine::new(replica, Arc::clone(&world.dataset), journal, clock);
            engine.sync_once().await.expect("first");
            world.dataset.put("edited.txt", b"mine");
            world.dataset.remove("deleted.txt");
            world.dataset.put("new.txt", b"mine");
            world.remote_edit(&edited, b"v2").await;
            engine.sync_once().await.expect("second")
        };
        let kept = world.dataset.get("edited.txt");
        let (puts, removes, conflicts) = (world.puts(), report.removed, report.conflicts.len());
        let want = match pull_only {
            // The edit is the server's now; nothing was sent; the file the person deleted and
            // the one they added are left as they are.
            true => (Some(b"v2".to_vec()), 0, 0, 0),
            // The edit crossed the server's: a stored conflict, the local copy kept, the
            // addition uploaded, the deletion sent.
            false => (Some(b"mine".to_vec()), 1, 1, 1),
        };
        assert_eq!(
            (kept, puts, removes, conflicts),
            want,
            "pull_only={pull_only}"
        );
    }
}
