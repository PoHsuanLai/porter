use super::*;
use crate::testing::scratch;
use porter_core::{Bytes, UnixSeconds};
use porter_sync::{
    Acknowledgement, Anchor, BaseVersion, Conflict, ContentHash, ItemPath, ItemState, RemoteSide,
    RemoteVersion, Tombstone, TombstoneOrigin,
};

fn item(local: &str, remote: Option<&str>, state: ItemState) -> JournalItem {
    JournalItem {
        local: LocalId(local.into()),
        remote: remote.map(|r| RemoteId(r.into())),
        path: ItemPath(format!("dir/{local}")),
        size: Bytes(7),
        hash: Some(ContentHash("abc".into())),
        remote_version: remote.map(|_| RemoteVersion("v1".into())),
        base: remote.map_or(BaseVersion::Absent, |_| {
            BaseVersion::At(RemoteVersion("v1".into()))
        }),
        state,
    }
}

fn tombstone(id: &str, ack: Acknowledgement) -> StoredTombstone {
    StoredTombstone {
        tombstone: Tombstone {
            id: RemoteId(id.into()),
            version: RemoteVersion("v9".into()),
            deleted_at: UnixSeconds(100),
        },
        origin: TombstoneOrigin::Local,
        ack,
    }
}

fn conflict(local: &str) -> StoredConflict {
    StoredConflict {
        number: None,
        conflict: Conflict {
            item: RemoteId("r1".into()),
            base: BaseVersion::Absent,
            remote: RemoteSide::Exists(RemoteId("r1".into()), RemoteVersion("v3".into())),
        },
        local: LocalId(local.into()),
        local_hash: Some(ContentHash("h".into())),
        at: UnixSeconds(5),
    }
}

#[test]
fn every_kind_of_row_round_trips() {
    let dir = scratch("rows");
    let journal = Journal::open(&dir.join("sync/acct/files.sqlite")).expect("open");
    let anchor = StoredAnchor {
        anchor: Anchor("seq-4".into()),
        at: UnixSeconds(42),
    };
    journal
        .apply(&[
            Op::PutItem(item("a", Some("r1"), ItemState::Synced)),
            Op::PutItem(item("b", None, ItemState::PendingUpload)),
            Op::SetAnchor(anchor.clone()),
            Op::PutTombstone(tombstone("r8", Acknowledgement::Pending)),
            Op::AddConflict(conflict("b")),
        ])
        .expect("apply");
    assert_eq!(
        journal.items().expect("items"),
        vec![
            item("a", Some("r1"), ItemState::Synced),
            item("b", None, ItemState::PendingUpload)
        ]
    );
    assert_eq!(journal.anchor().expect("anchor"), Some(anchor));
    assert_eq!(
        journal.tombstones().expect("tombstones"),
        vec![tombstone("r8", Acknowledgement::Pending)]
    );
    let conflicts = journal.conflicts().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].number, Some(1));
    assert_eq!(conflicts[0].conflict, conflict("b").conflict);
    assert_eq!(conflicts[0].local_hash, Some(ContentHash("h".into())));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn put_replaces_by_local_id_and_the_other_ops_undo_their_rows() {
    let dir = scratch("ops");
    let journal = Journal::open(&dir.join("j.sqlite")).expect("open");
    journal
        .apply(&[
            Op::PutItem(item("a", None, ItemState::PendingUpload)),
            Op::SetAnchor(StoredAnchor {
                anchor: Anchor("x".into()),
                at: UnixSeconds(1),
            }),
            Op::PutTombstone(tombstone("r1", Acknowledgement::Pending)),
            Op::PutTombstone(tombstone("r2", Acknowledgement::Pending)),
            Op::AddConflict(conflict("a")),
        ])
        .expect("first");
    journal
        .apply(&[
            Op::PutItem(item("a", Some("r1"), ItemState::Synced)),
            Op::SetAnchor(StoredAnchor {
                anchor: Anchor("y".into()),
                at: UnixSeconds(2),
            }),
            Op::Acknowledge(RemoteId("r1".into())),
            Op::DropConflict(1),
        ])
        .expect("second");
    assert_eq!(
        journal.items().expect("items"),
        vec![item("a", Some("r1"), ItemState::Synced)]
    );
    assert_eq!(
        journal.anchor().expect("anchor").map(|a| a.anchor.0),
        Some("y".into())
    );
    assert!(journal.conflicts().expect("conflicts").is_empty());
    journal.apply(&[Op::CompactTombstones]).expect("compact");
    let left = journal.tombstones().expect("tombstones");
    assert_eq!(left, vec![tombstone("r2", Acknowledgement::Pending)]);
    journal
        .apply(&[Op::ClearAnchor, Op::DeleteItem(LocalId("a".into()))])
        .expect("clear");
    assert_eq!(journal.anchor().expect("anchor"), None);
    assert!(journal.items().expect("items").is_empty());
    assert_eq!(journal.writes(), 4);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_cut_write_leaves_none_of_its_rows_and_a_reopened_file_holds_what_committed() {
    let dir = scratch("cut");
    let path = dir.join("j.sqlite");
    let journal = Journal::open(&path).expect("open");
    journal
        .apply(&[Op::PutItem(item("a", None, ItemState::PendingUpload))])
        .expect("first");
    journal.cut_at(0);
    let refused = journal.apply(&[
        Op::PutItem(item("b", None, ItemState::PendingUpload)),
        Op::ClearAnchor,
    ]);
    assert!(matches!(refused, Err(JournalError::Cut)));
    drop(journal);
    let again = Journal::open(&path).expect("reopen");
    assert_eq!(
        again.items().expect("items"),
        vec![item("a", None, ItemState::PendingUpload)]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A version-1 file exactly as the first syncd wrote it, kept as text so the schema may move on.
const V1_FILE: &str = "
CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY NOT NULL, applied_at INTEGER NOT NULL);
INSERT INTO schema_migrations VALUES (1, 1700000000);
CREATE TABLE items (local_id TEXT PRIMARY KEY NOT NULL, remote_id TEXT UNIQUE, path TEXT NOT NULL,
    size INTEGER NOT NULL, hash TEXT, remote_version TEXT, base_version TEXT, state TEXT NOT NULL);
CREATE TABLE anchors (id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1), anchor TEXT NOT NULL,
    updated_at INTEGER NOT NULL);
CREATE TABLE tombstones (remote_id TEXT PRIMARY KEY NOT NULL, version TEXT NOT NULL,
    deleted_at INTEGER NOT NULL, origin TEXT NOT NULL, ack TEXT NOT NULL);
CREATE TABLE conflicts (number INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, item TEXT NOT NULL,
    base TEXT, remote TEXT NOT NULL, local_id TEXT NOT NULL, local_hash TEXT, at INTEGER NOT NULL);
INSERT INTO items VALUES ('a', 'r1', 'dir/a', 7, 'abc', 'v1', 'v1', 'synced');
INSERT INTO anchors VALUES (1, 'seq-3', 50);
INSERT INTO tombstones VALUES ('r9', 'v9', 100, 'remote', 'acknowledged');
INSERT INTO conflicts VALUES (1, 'r1', NULL, '{\"kind\":\"changed\",\"v\":\"v2\"}', 'a', 'abc', 60);
";

fn v1_file(name: &str) -> std::path::PathBuf {
    let dir = scratch(name);
    let path = dir.join("v1.sqlite");
    Connection::open(&path)
        .expect("create")
        .execute_batch(V1_FILE)
        .expect("v1 schema");
    path
}

#[test]
fn a_version_1_file_opens_and_reads_back() {
    let path = v1_file("v1");
    let journal = Journal::open(&path).expect("a v1 file opens");
    assert_eq!(journal.version().expect("version"), 1);
    assert_eq!(
        journal.items().expect("items"),
        vec![item("a", Some("r1"), ItemState::Synced)]
    );
    assert_eq!(
        journal.anchor().expect("anchor").map(|a| a.at),
        Some(UnixSeconds(50))
    );
    let tombstones = journal.tombstones().expect("tombstones");
    assert_eq!(tombstones[0].ack, Acknowledgement::Acknowledged);
    assert_eq!(tombstones[0].origin, TombstoneOrigin::Remote);
    let conflicts = journal.conflicts().expect("conflicts");
    assert_eq!(
        conflicts[0].conflict.remote,
        RemoteSide::Changed(RemoteVersion("v2".into()))
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
}

#[test]
fn a_file_is_brought_forward_one_migration_at_a_time_and_a_newer_one_is_refused() {
    let path = v1_file("forward");
    let migrations = [
        schema::MIGRATIONS[0],
        "ALTER TABLE items ADD COLUMN note TEXT;",
    ];
    let forward = Journal::open_with(&path, &migrations).expect("v2 build opens a v1 file");
    assert_eq!(forward.version().expect("version"), 2);
    assert_eq!(forward.items().expect("items").len(), 1);
    drop(forward);
    let behind = Journal::open(&path);
    assert!(
        matches!(behind, Err(JournalError::Newer { found: 2 })),
        "{behind:?}"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
}

#[test]
fn a_row_this_build_cannot_read_is_corruption_not_a_panic() {
    let dir = scratch("corrupt");
    let path = dir.join("j.sqlite");
    drop(Journal::open(&path).expect("open"));
    Connection::open(&path)
        .expect("raw")
        .execute_batch(
            "INSERT INTO items VALUES ('a', NULL, 'p', 1, NULL, NULL, NULL, 'sideways');",
        )
        .expect("insert");
    let journal = Journal::open(&path).expect("reopen");
    assert!(matches!(journal.items(), Err(JournalError::Corrupt(_))));
    let _ = std::fs::remove_dir_all(dir);
}
