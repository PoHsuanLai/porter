use super::*;
use crate::datasets::pim::items_in;
use crate::journal::Journal;
use crate::testing::scratch;
use porter_core::UnixSeconds;
use porter_sync::{
    Anchor, BaseVersion, Conflict, RemoteId, RemoteSide, RemoteVersion, StoredAnchor,
    StoredConflict,
};

pub(super) fn ics(uid: &str, summary: &str) -> Blob {
    Blob(
        format!("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")
            .into_bytes(),
    )
}

fn open(name: &str) -> (PimMirror, Journal, PathBuf) {
    let dir = scratch(name);
    let journal = Journal::open(&dir.join("j.sqlite")).expect("journal");
    let root = dir.join("vdir/a1/personal");
    let mirror = PimMirror::open(
        DatasetId::parse("pim_cal_personal").expect("id"),
        PimKind::Calendar,
        root,
        &journal,
    )
    .expect("open");
    (mirror, journal, dir)
}

fn path(name: &str) -> ItemPath {
    ItemPath(name.to_owned())
}

#[tokio::test]
async fn an_item_is_stored_under_its_uid_read_back_and_discarded() {
    let (mirror, _journal, dir) = open("store");
    let stored = mirror
        .store(None, &path("server-name.ics"), ics("abc-1", "one"))
        .await
        .expect("store");
    assert_eq!(stored.local, LocalId("abc-1.ics".into()));
    assert_eq!(stored.path, path("server-name.ics"));
    assert_eq!(stored.hash, fingerprint(&ics("abc-1", "one").0));
    assert_eq!(
        mirror.read(&stored.local).await.expect("read"),
        ics("abc-1", "one")
    );
    assert_eq!(
        mirror.scan().await.expect("scan"),
        std::slice::from_ref(&stored)
    );

    // The server changes it: the same file is replaced whole.
    let again = mirror
        .store(
            Some(&stored.local),
            &path("server-name.ics"),
            ics("abc-1", "two"),
        )
        .await
        .expect("store");
    assert_eq!(again.local, stored.local);
    assert_eq!(items_in(mirror.root(), PimKind::Calendar), ["abc-1.ics"]);

    mirror.discard(&stored.local).await.expect("discard");
    mirror.discard(&stored.local).await.expect("again");
    assert!(items_in(mirror.root(), PimKind::Calendar).is_empty());
    assert!(mirror.scan().await.expect("scan").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn storing_what_is_already_there_does_not_touch_the_file() {
    let (mirror, _journal, dir) = open("idem");
    let first = mirror
        .store(None, &path("a.ics"), ics("u", "x"))
        .await
        .expect("store");
    let file = mirror.root().join(&first.local.0);
    let before = std::fs::metadata(&file).expect("m").modified().expect("t");
    std::thread::sleep(std::time::Duration::from_millis(30));
    mirror
        .store(Some(&first.local), &path("a.ics"), ics("u", "x"))
        .await
        .expect("store");
    assert_eq!(
        std::fs::metadata(&file).expect("m").modified().expect("t"),
        before
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_uid_that_changes_moves_the_file_and_a_shared_uid_keeps_both_items() {
    let (mirror, _journal, dir) = open("names");
    let first = mirror
        .store(None, &path("a.ics"), ics("one", "x"))
        .await
        .expect("store");
    let moved = mirror
        .store(Some(&first.local), &path("a.ics"), ics("two", "x"))
        .await
        .expect("store");
    assert_eq!(moved.local, LocalId("two.ics".into()));
    assert_eq!(
        items_in(mirror.root(), PimKind::Calendar),
        ["two.ics"],
        "the old file is gone"
    );

    // Two server files claim one UID: the second is named as the server names it, a third that
    // would also clash gets the hash of its path.
    let second = mirror
        .store(None, &path("b.ics"), ics("two", "y"))
        .await
        .expect("store");
    assert_eq!(second.local, LocalId("b.ics".into()));
    let third = mirror
        .store(None, &path("sub/b.ics"), ics("two", "z"))
        .await
        .expect("store");
    assert!(
        third.local.0.starts_with("b-") && third.local.0.ends_with(".ics"),
        "{third:?}"
    );
    assert_eq!(mirror.scan().await.expect("scan").len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_local_edit_or_deletion_is_never_reported_as_a_change() {
    let (mirror, _journal, dir) = open("readonly");
    let small = mirror
        .store(None, &path("s.ics"), ics("s", "small"))
        .await
        .expect("store");
    let big_text = format!("{}{}", "X".repeat(ITEM_CAP), "tail");
    let big_blob = Blob(
        format!("BEGIN:VCALENDAR\r\nUID:big\r\nDESCRIPTION:{big_text}\r\nEND:VCALENDAR\r\n")
            .into_bytes(),
    );
    let big = mirror
        .store(None, &path("big.ics"), big_blob.clone())
        .await
        .expect("store");
    let before = mirror.scan().await.expect("scan");

    std::fs::write(mirror.root().join(&small.local.0), "edited").expect("edit");
    std::fs::write(mirror.root().join(&big.local.0), "edited").expect("edit");
    std::fs::write(mirror.root().join("stray.ics"), "new file").expect("stray");
    let after = mirror.scan().await.expect("scan");
    assert_eq!(after, before, "what the mirror stored, not what is on disk");
    // The small one was written back at once, the big one stays edited and is not listed as new.
    assert_eq!(
        mirror.read(&small.local).await.expect("read"),
        ics("s", "small")
    );
    assert_eq!(
        std::fs::read(mirror.root().join(&big.local.0)).expect("big"),
        b"edited"
    );

    std::fs::remove_file(mirror.root().join(&small.local.0)).expect("delete");
    std::fs::remove_file(mirror.root().join(&big.local.0)).expect("delete");
    assert_eq!(
        mirror.scan().await.expect("scan"),
        before,
        "a deletion is no removal either"
    );
    assert!(
        mirror.root().join(&small.local.0).exists(),
        "the small one is back"
    );
    assert!(
        !mirror.root().join(&big.local.0).exists(),
        "the big one waits for the server"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn nothing_outside_the_collection_can_be_read_or_discarded() {
    let (mirror, _journal, dir) = open("escape");
    std::fs::write(dir.join("secret"), "s").expect("secret");
    for name in ["../../../secret", "/etc/passwd", ".hidden", "a/b.ics", ""] {
        let id = LocalId(name.to_owned());
        assert!(mirror.read(&id).await.is_err(), "{name:?}");
        assert!(mirror.discard(&id).await.is_err(), "{name:?}");
    }
    assert!(dir.join("secret").exists());
    let _ = std::fs::remove_dir_all(dir);
}

fn row(local: &str, hash: Option<&ContentHash>, state: ItemState) -> porter_sync::JournalItem {
    porter_sync::JournalItem {
        local: LocalId(local.to_owned()),
        remote: Some(RemoteId(format!("/c/{local}"))),
        path: path(local),
        size: Bytes(1),
        hash: hash.cloned(),
        remote_version: Some(RemoteVersion("v1".into())),
        base: BaseVersion::At(RemoteVersion("v1".into())),
        state,
    }
}

#[tokio::test]
async fn opening_queues_every_damaged_item_to_be_fetched_again_and_nothing_else() {
    let dir = scratch("heal");
    let journal = Journal::open(&dir.join("j.sqlite")).expect("journal");
    let root = dir.join("vdir/a1/personal");
    std::fs::create_dir_all(&root).expect("root");
    let good = ics("good", "g");
    let edited = ics("edited", "e");
    for (name, blob) in [
        ("good.ics", &good),
        ("edited.ics", &edited),
        ("conflicted.ics", &good),
    ] {
        std::fs::write(root.join(name), &blob.0).expect("file");
    }
    std::fs::write(root.join("edited.ics"), "someone's edit").expect("edit");
    let ok = fingerprint(&good.0);
    let conflict = StoredConflict {
        number: None,
        conflict: Conflict {
            item: RemoteId("/c/conflicted.ics".into()),
            base: BaseVersion::Absent,
            remote: RemoteSide::Changed(RemoteVersion("v2".into())),
        },
        local: LocalId("conflicted.ics".into()),
        local_hash: Some(ok.clone()),
        at: UnixSeconds(1),
    };
    journal
        .apply(&[
            Op::PutItem(row("good.ics", Some(&ok), ItemState::Synced)),
            Op::PutItem(row(
                "edited.ics",
                Some(&fingerprint(&edited.0)),
                ItemState::Synced,
            )),
            Op::PutItem(row("missing.ics", Some(&ok), ItemState::Synced)),
            Op::PutItem(row("conflicted.ics", Some(&ok), ItemState::Conflicted)),
            Op::PutItem(row("fetching.ics", None, ItemState::Fetching)),
            Op::AddConflict(conflict),
            Op::SetAnchor(StoredAnchor {
                anchor: Anchor("sync:1".into()),
                at: UnixSeconds(5),
            }),
        ])
        .expect("rows");

    let mirror = PimMirror::open(
        DatasetId::parse("pim_cal_personal").expect("id"),
        PimKind::Calendar,
        root,
        &journal,
    )
    .expect("open");
    assert_eq!(mirror.healed(), 3);
    let mut left: Vec<String> = journal
        .items()
        .expect("items")
        .into_iter()
        .map(|r| r.local.0)
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["fetching.ics", "good.ics"],
        "the engine finishes the fetching one itself"
    );
    assert_eq!(
        journal.anchor().expect("anchor"),
        None,
        "so the next cycle lists again"
    );
    assert!(journal.conflicts().expect("conflicts").is_empty());
    let scanned: Vec<String> = mirror
        .scan()
        .await
        .expect("scan")
        .into_iter()
        .map(|s| s.local.0)
        .collect();
    assert_eq!(scanned, ["good.ics"]);

    // A clean journal is left alone, anchor included.
    journal
        .apply(&[Op::SetAnchor(StoredAnchor {
            anchor: Anchor("sync:2".into()),
            at: UnixSeconds(6),
        })])
        .expect("anchor");
    let again = PimMirror::open(
        DatasetId::parse("pim_cal_personal").expect("id"),
        PimKind::Calendar,
        mirror.root().to_owned(),
        &journal,
    )
    .expect("open");
    assert_eq!(again.healed(), 0);
    assert!(journal.anchor().expect("anchor").is_some());
    let _ = std::fs::remove_dir_all(dir);
}
