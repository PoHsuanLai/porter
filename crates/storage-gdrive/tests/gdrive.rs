//! What is the Drive replica's own, against the fake Google: the content hash, paths from
//! folders, remote renames and moves, trash and permanent delete, the first listing in pages and
//! the anchors that come out of it, multipart and resumable uploads on the wire, a restart that
//! resumes from an anchor, throttling, and that nothing leaves the app data folder.

mod common;

use common::{Server, TOKEN};
use porter_fake_servers::google::md5_hex;
use porter_sync::{
    Anchor, BaseVersion, Blob, ByteRange, Change, Cursor, ItemPath, More, PutItem, PutRefused,
    PutTarget, Replica as _, ReplicaError, RetryAfter,
};
use storage_gdrive::{CHUNK_UNIT, Uploads};

fn new_item(path: &str, bytes: &[u8]) -> PutItem {
    PutItem {
        target: PutTarget::New(ItemPath(path.into())),
        content: Blob(bytes.to_vec()),
        hash: None,
    }
}

/// Everything the first listing holds, following the pages.
async fn listing(replica: &impl porter_sync::Replica) -> (Vec<Change>, Anchor) {
    let mut cursor = Cursor::Start;
    let mut all = Vec::new();
    loop {
        let page = replica.changes(cursor).await.expect("page");
        all.extend(page.changes);
        match page.more {
            More::More => cursor = Cursor::At(page.next),
            More::Done => return (all, page.next),
        }
    }
}

fn upserts(changes: &[Change]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) => Some((item.path.0.clone(), item.id.0.clone())),
            Change::Tombstone(_) => None,
        })
        .collect();
    found.sort();
    found
}

#[tokio::test]
async fn an_item_carries_the_md5_drive_reports_and_its_size() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::default());
    let (_, anchor) = listing(&replica).await;
    replica
        .put(new_item("a.jpg", b"abc"), BaseVersion::Absent)
        .await
        .expect("put");
    let page = replica.changes(Cursor::At(anchor)).await.expect("changes");
    let [Change::Upsert(item)] = page.changes.as_slice() else {
        panic!("{page:?}")
    };
    assert_eq!(
        item.hash.as_ref().map(|h| h.0.as_str()),
        Some(md5_hex(b"abc").as_str())
    );
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(item.size.0, 3);
    assert_eq!(item.path.0, "a.jpg");
}

#[tokio::test]
async fn folders_make_paths_and_a_remote_rename_or_move_is_the_same_item_at_a_new_path() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::default());
    server.google.drive_put_file("2026/09/a.jpg", b"one");
    server.google.drive_put_file("b.jpg", b"two");
    let (all, anchor) = listing(&replica).await;
    let first = upserts(&all);
    assert_eq!(
        first.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        ["2026/09/a.jpg", "b.jpg"]
    );
    let moved = first[0].1.clone();

    server.google.drive_move("2026/09/a.jpg", "2026/10/c.jpg");
    server.google.drive_move("b.jpg", "b2.jpg");
    let page = replica.changes(Cursor::At(anchor)).await.expect("changes");
    let got = upserts(&page.changes);
    assert_eq!(
        got.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        ["2026/10/c.jpg", "b2.jpg"]
    );
    assert_eq!(got[0].1, moved, "a move keeps the item's id");
}

#[tokio::test]
async fn a_delete_a_trash_and_a_deleted_folder_are_tombstones() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::default());
    server.google.drive_put_file("keep.txt", b"k");
    server.google.drive_put_file("gone.txt", b"g");
    server.google.drive_put_file("trashed.txt", b"t");
    server.google.drive_put_file("dir/in.txt", b"i");
    let (all, anchor) = listing(&replica).await;
    let ids: std::collections::HashMap<String, String> = upserts(&all).into_iter().collect();

    server.google.drive_delete("gone.txt");
    server.google.drive_trash("trashed.txt");
    server.google.drive_delete("dir");
    let page = replica.changes(Cursor::At(anchor)).await.expect("changes");
    let mut tombstoned: Vec<String> = page
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Tombstone(t) => Some(t.id.0.clone()),
            Change::Upsert(_) => None,
        })
        .collect();
    tombstoned.sort();
    let mut want = vec![
        ids["gone.txt"].clone(),
        ids["trashed.txt"].clone(),
        ids["dir/in.txt"].clone(),
    ];
    want.sort();
    // The folder's own removal tombstones the file the replica saw in it.
    assert!(
        want.iter().all(|id| tombstoned.contains(id)),
        "{tombstoned:?} vs {want:?}"
    );
    assert!(!tombstoned.contains(&ids["keep.txt"]));
}

#[tokio::test]
async fn the_first_listing_pages_and_its_anchors_resume_in_a_new_process() {
    let server = Server::start().await;
    for name in ["a", "b", "c", "d", "e"] {
        server.google.drive_put_file(name, name.as_bytes());
    }
    let replica = server.replica("", 2, Uploads::default());
    let first = replica.changes(Cursor::Start).await.expect("page 1");
    assert_eq!((first.changes.len(), first.more), (2, More::More));
    assert!(first.next.0.starts_with("list:"), "{:?}", first.next);

    // A new process picks the listing up from the anchor alone.
    let again = server.replica("", 2, Uploads::default());
    let second = again.changes(Cursor::At(first.next)).await.expect("page 2");
    assert_eq!((second.changes.len(), second.more), (2, More::More));
    let third = again
        .changes(Cursor::At(second.next))
        .await
        .expect("page 3");
    assert_eq!((third.changes.len(), third.more), (1, More::Done));
    assert!(third.next.0.starts_with("changes:"), "{:?}", third.next);

    // A change made while the listing ran is not missed: the start token was taken first.
    server.google.drive_put_file("f", b"f");
    let later = server
        .replica("", 2, Uploads::default())
        .changes(Cursor::At(third.next))
        .await
        .expect("changes");
    assert_eq!(upserts(&later.changes).len(), 1);
}

#[tokio::test]
async fn an_anchor_that_is_not_ours_or_that_drive_refuses_is_expired() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::default());
    for text in [
        "https://graph.microsoft.com/v1.0/delta?token=1",
        "changes:",
        "list:1:",
        "nonsense",
        "changes:99999",
    ] {
        assert_eq!(
            replica.changes(Cursor::At(Anchor(text.into()))).await,
            Err(ReplicaError::AnchorExpired),
            "{text}"
        );
    }
}

#[tokio::test]
async fn a_small_file_is_one_multipart_request_and_a_large_one_a_resumable_session() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::new(1_000, 2 * CHUNK_UNIT));
    replica.changes(Cursor::Start).await.expect("listing");
    let before = server.google.hits().len();

    let (small_id, _) = replica
        .put(new_item("small.bin", &[7; 900]), BaseVersion::Absent)
        .await
        .expect("small");
    let wire: Vec<String> = server.google.hits()[before..]
        .iter()
        .map(|h| format!("{} {}", h.method, h.target.split('?').next().unwrap_or("")))
        .collect();
    assert!(
        wire.contains(&"POST /upload/drive/v3/files".to_owned()),
        "{wire:?}"
    );
    assert!(
        !wire.iter().any(|w| w.starts_with("PUT")),
        "no session for a small file: {wire:?}"
    );

    // A large new file: 5 chunks' worth, the last one short.
    let big: Vec<u8> = (0..(4 * CHUNK_UNIT + 1234))
        .map(|i| (i % 251) as u8)
        .collect();
    let mark = server.google.hits().len();
    let (big_id, _) = replica
        .put(new_item("dir/big.bin", &big), BaseVersion::Absent)
        .await
        .expect("big");
    let hits = &server.google.hits()[mark..];
    let sessions: Vec<_> = hits
        .iter()
        .filter(|h| h.target.contains("uploadType=resumable"))
        .collect();
    let statuses: Vec<u16> = sessions.iter().map(|h| h.status).collect();
    // The start, then chunks of two units, two units and 1234 bytes.
    assert_eq!(statuses, [200, 308, 308, 200], "{sessions:?}");
    assert_eq!(server.google.drive_file("dir/big.bin"), Some(big.clone()));
    assert_eq!(
        replica.fetch(&big_id, ByteRange::Whole).await,
        Ok(Blob(big.clone()))
    );

    // A changed large file goes the same way, over the id, and a stale base is not sent at all.
    let bigger: Vec<u8> = big.iter().rev().copied().collect();
    let mark = server.google.hits().len();
    let (_, next) = replica
        .put(
            PutItem {
                target: PutTarget::Existing(big_id.clone()),
                content: Blob(bigger.clone()),
                hash: None,
            },
            BaseVersion::At(server_version(&server, "dir/big.bin")),
        )
        .await
        .expect("rewrite");
    assert_eq!(server.google.drive_file("dir/big.bin"), Some(bigger));
    assert_eq!(server.google.drive_version("dir/big.bin"), Some(next.0));
    let override_seen = server.google.hits()[mark..].iter().any(|h| {
        h.method == "POST" && h.target.contains("/files/") && h.target.contains("resumable")
    });
    assert!(override_seen);

    let mark = server.google.hits().len();
    let refused = replica
        .put(
            PutItem {
                target: PutTarget::Existing(small_id),
                content: Blob(vec![1; 5_000]),
                hash: None,
            },
            BaseVersion::At(porter_sync::RemoteVersion("0".into())),
        )
        .await;
    assert!(
        matches!(refused, Err(PutRefused::Conflict(_))),
        "{refused:?}"
    );
    assert!(
        server.google.hits()[mark..]
            .iter()
            .all(|h| !h.target.contains("upload")),
        "a stale base sends no content"
    );
}

fn server_version(server: &Server, path: &str) -> porter_sync::RemoteVersion {
    porter_sync::RemoteVersion(server.google.drive_version(path).expect("version"))
}

#[tokio::test]
async fn a_remote_edit_between_listing_and_write_is_a_conflict_and_nothing_is_overwritten() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::default());
    server.google.drive_put_file("doc.txt", b"one");
    let (all, _) = listing(&replica).await;
    let Some(Change::Upsert(item)) = all.into_iter().next() else {
        panic!("one item")
    };
    server.google.drive_put_file("doc.txt", b"theirs");
    let refused = replica
        .put(
            PutItem {
                target: PutTarget::Existing(item.id.clone()),
                content: Blob(b"mine".to_vec()),
                hash: None,
            },
            BaseVersion::At(item.version.clone()),
        )
        .await;
    assert!(
        matches!(refused, Err(PutRefused::Conflict(_))),
        "{refused:?}"
    );
    assert_eq!(
        server.google.drive_file("doc.txt"),
        Some(b"theirs".to_vec())
    );
    // And a removal on the stale base is refused the same way.
    let removed = replica
        .remove(&item.id, BaseVersion::At(item.version))
        .await;
    assert!(
        matches!(removed, Err(PutRefused::Conflict(_))),
        "{removed:?}"
    );
    assert_eq!(
        server.google.drive_file("doc.txt"),
        Some(b"theirs".to_vec())
    );
}

#[tokio::test]
async fn a_dataset_folder_sees_only_itself_and_makes_itself() {
    let server = Server::start().await;
    server.google.drive_put_file("Other/skip.txt", b"x");
    server
        .google
        .drive_put_file("Photos/Originals/a/p.jpg", b"p");
    let replica = server.replica("Photos/Originals", 10, Uploads::default());
    let (all, anchor) = listing(&replica).await;
    assert_eq!(
        upserts(&all)
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>(),
        ["a/p.jpg"]
    );
    server.google.drive_put_file("Other/skip2.txt", b"y");
    server.google.drive_put_file("Photos/Originals/q.jpg", b"q");
    let page = replica.changes(Cursor::At(anchor)).await.expect("changes");
    assert_eq!(
        upserts(&page.changes)
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>(),
        ["q.jpg"]
    );

    // A folder that is not there yet is made by the first listing and written into by a put.
    let fresh = server.replica("Photos/Metadata", 10, Uploads::default());
    let (none, _) = listing(&fresh).await;
    assert!(none.is_empty());
    fresh
        .put(new_item("dev.json", b"{}"), BaseVersion::Absent)
        .await
        .expect("put");
    assert_eq!(
        server.google.drive_file("Photos/Metadata/dev.json"),
        Some(b"{}".to_vec())
    );
}

#[tokio::test]
async fn a_throttled_request_is_a_retry_with_the_servers_wait_and_a_bad_path_is_refused() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::default());
    server.google.throttle(1, 17);
    assert_eq!(
        replica.quota().await,
        Err(ReplicaError::Transient(RetryAfter(17)))
    );
    assert!(replica.quota().await.is_ok());
    server.google.throttle(1, 9);
    assert_eq!(
        replica
            .put(new_item("a.txt", b"x"), BaseVersion::Absent)
            .await,
        Err(PutRefused::Transient(RetryAfter(9)))
    );
    for path in ["", "/a", "a//b", "../a"] {
        assert_eq!(
            replica.put(new_item(path, b"x"), BaseVersion::Absent).await,
            Err(PutRefused::Forbidden),
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn every_request_carries_the_relays_bearer_and_the_replica_adds_none_of_its_own() {
    let server = Server::start().await;
    let replica = server.replica("", 10, Uploads::new(10, 1));
    let (_, anchor) = listing(&replica).await;
    let (id, version) = replica
        .put(new_item("a.bin", &vec![1; 500_000]), BaseVersion::Absent)
        .await
        .expect("put");
    replica.changes(Cursor::At(anchor)).await.expect("changes");
    replica.fetch(&id, ByteRange::Whole).await.expect("fetch");
    replica
        .remove(&id, BaseVersion::At(version))
        .await
        .expect("remove");
    let want = format!("Bearer {TOKEN}");
    let hits = server.google.hits();
    assert!(hits.len() > 8);
    assert!(
        hits.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str())),
        "{hits:?}"
    );
    // The app data folder only: no request names a space or a drive but its own.
    assert!(hits.iter().all(|h| !h.target.contains("spaces=drive")));
}
