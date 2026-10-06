//! `GraphReplica` against the fake Graph drive: what the contract suite does not say. The
//! dataset's folder inside the app folder, deleted folders, the upload threshold and chunks,
//! conditions on sessions, downloads, throttling, the credential, and anchor expiry that uploads
//! nothing again.

mod common;

use common::{Replica, Server, TOKEN, TcpDial};
use porter_core::{Bytes, WebUrl};
use porter_fake_servers::graph::{CHUNK_UNIT, Knobs};
use porter_fake_servers::{GraphHandle, Hit};
use porter_sync::{
    BaseVersion, Blob, ByteRange, Change, Conflict, Cursor, ItemPath, PutItem, PutRefused,
    PutTarget, RemoteId, RemoteSide, RemoteVersion, Replica as _, ReplicaError, RetryAfter,
};
use storage_graph::{Clock, GraphReplica, StreamHttp, StreamLimits, Uploads};

fn new_item(path: &str, bytes: &[u8]) -> PutItem {
    PutItem {
        target: PutTarget::New(ItemPath(path.into())),
        content: Blob(bytes.to_vec()),
        hash: None,
    }
}

fn change_to(id: &RemoteId, bytes: &[u8]) -> PutItem {
    PutItem {
        target: PutTarget::Existing(id.clone()),
        content: Blob(bytes.to_vec()),
        hash: None,
    }
}

fn writes(graph: &GraphHandle) -> Vec<Hit> {
    graph
        .hits()
        .into_iter()
        .filter(|h| matches!(h.method.as_str(), "PUT" | "POST" | "DELETE"))
        .collect()
}

fn upserts(page: &porter_sync::ChangePage) -> Vec<(String, u64)> {
    page.changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) => Some((item.path.0.clone(), item.size.0)),
            Change::Tombstone(_) => None,
        })
        .collect()
}

#[tokio::test]
async fn a_dataset_folder_is_made_on_the_first_listing_and_only_its_files_are_seen() {
    let server = Server::start().await;
    server.graph.put_file("Photos/other.jpg", b"not ours");
    let replica = server.replica("Photos/Originals", 50, Uploads::default());

    let listing = replica.changes(Cursor::Start).await.expect("listing");
    assert_eq!(upserts(&listing), vec![]);
    let made: Vec<String> = server
        .graph
        .hits()
        .iter()
        .filter(|h| h.method == "POST")
        .map(|h| format!("{} {}", h.target, h.status))
        .collect();
    // `Photos` existed (409), `Originals` was made.
    assert_eq!(
        made,
        vec![
            "/v1.0/me/drive/special/approot/children 409".to_owned(),
            "/v1.0/me/drive/special/approot:/Photos:/children 201".to_owned(),
        ]
    );

    let (id, _) = replica
        .put(new_item("2026/09/a b+c#d.jpg", b"one"), BaseVersion::Absent)
        .await
        .expect("put");
    assert_eq!(
        server.graph.file("Photos/Originals/2026/09/a b+c#d.jpg"),
        Some(b"one".to_vec())
    );
    server.graph.put_file("Photos/other2.jpg", b"also not ours");
    server.graph.put_file("Photos/Originals/top.jpg", b"top");
    let page = replica
        .changes(Cursor::At(listing.next))
        .await
        .expect("changes");
    assert_eq!(
        upserts(&page),
        vec![
            ("2026/09/a b+c#d.jpg".to_owned(), 3),
            ("top.jpg".to_owned(), 3)
        ]
    );
    assert_eq!(
        replica.fetch(&id, ByteRange::Whole).await,
        Ok(Blob(b"one".to_vec()))
    );
}

#[tokio::test]
async fn a_deleted_folder_is_one_change_and_the_files_it_held_get_tombstones() {
    let server = Server::start().await;
    server.graph.put_file("a/1.jpg", b"1");
    server.graph.put_file("a/b/2.jpg", b"2");
    server.graph.put_file("keep.jpg", b"k");
    let replica = server.replica("", 50, Uploads::default());
    let listing = replica.changes(Cursor::Start).await.expect("listing");
    assert_eq!(upserts(&listing).len(), 3);

    server.graph.delete("a");
    let page = replica
        .changes(Cursor::At(listing.next))
        .await
        .expect("changes");
    let mut gone: Vec<String> = page
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Tombstone(t) => Some(t.id.0.clone()),
            Change::Upsert(_) => None,
        })
        .collect();
    let mut held: Vec<String> = listing
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) if item.path.0.starts_with("a/") => Some(item.id.0.clone()),
            _ => None,
        })
        .collect();
    gone.sort();
    held.sort();
    assert_eq!(held.len(), 2);
    assert_eq!(gone, held);
}

#[tokio::test]
async fn a_file_is_one_put_up_to_the_threshold_and_a_session_in_chunks_past_it() {
    let server = Server::start().await;
    let replica = server.replica("", 50, Uploads::new(10, CHUNK_UNIT));

    replica
        .put(new_item("small.bin", &[7; 10]), BaseVersion::Absent)
        .await
        .expect("small");
    let small: Vec<String> = writes(&server.graph)
        .iter()
        .map(|h| format!("{} {}", h.method, h.target))
        .collect();
    assert_eq!(
        small,
        vec![
            "PUT /v1.0/me/drive/special/approot:/small.bin:/content?@microsoft.graph.conflictBehavior=fail"
                .to_owned()
        ]
    );

    let big: Vec<u8> = (0..CHUNK_UNIT * 5 / 2).map(|i| (i % 251) as u8).collect();
    let (id, _) = replica
        .put(new_item("dir/big.bin", &big), BaseVersion::Absent)
        .await
        .expect("big");
    assert_eq!(server.graph.file("dir/big.bin"), Some(big.clone()));
    let after: Vec<Hit> = writes(&server.graph).into_iter().skip(1).collect();
    let shape: Vec<(&str, bool)> = after
        .iter()
        .map(|h| (h.method.as_str(), h.target.starts_with("/upload/")))
        .collect();
    assert_eq!(
        shape,
        vec![("POST", false), ("PUT", true), ("PUT", true), ("PUT", true)],
        "a session, then three chunks of 320 KiB x 1, x 1 and the rest"
    );
    assert_eq!(after.last().map(|h| h.status), Some(201));
    assert_eq!(
        after.iter().filter(|h| h.status == 202).count(),
        2,
        "every chunk but the last is accepted with 202"
    );
    // Ranges read back.
    let span = ByteRange::Span {
        start: Bytes(CHUNK_UNIT as u64 - 2),
        len: Bytes(4),
    };
    assert_eq!(
        replica.fetch(&id, span).await,
        Ok(Blob(big[CHUNK_UNIT - 2..CHUNK_UNIT + 2].to_vec()))
    );
}

#[tokio::test]
async fn a_session_carries_its_conditions_so_a_stale_or_taken_target_is_a_conflict() {
    let server = Server::start().await;
    let replica = server.replica("", 50, Uploads::new(0, CHUNK_UNIT));
    let (id, first) = replica
        .put(new_item("a.bin", b"one"), BaseVersion::Absent)
        .await
        .expect("create through a session");

    // Another device changes it; the session for the old version is refused at its start.
    server.graph.put_file("a.bin", b"theirs");
    let theirs = RemoteVersion(server.graph.etag("a.bin").expect("etag"));
    let refused = replica
        .put(change_to(&id, b"mine"), BaseVersion::At(first.clone()))
        .await;
    assert_eq!(
        refused,
        Err(PutRefused::Conflict(Conflict {
            item: id.clone(),
            base: BaseVersion::At(first),
            remote: RemoteSide::Changed(theirs.clone()),
        }))
    );
    assert_eq!(server.graph.file("a.bin"), Some(b"theirs".to_vec()));

    // A new item over a path that is taken.
    let taken = replica
        .put(new_item("a.bin", b"other"), BaseVersion::Absent)
        .await;
    assert_eq!(
        taken,
        Err(PutRefused::Conflict(Conflict {
            item: id.clone(),
            base: BaseVersion::Absent,
            remote: RemoteSide::Exists(id, theirs),
        }))
    );
    assert_eq!(server.graph.file("a.bin"), Some(b"theirs".to_vec()));
}

#[tokio::test]
async fn a_remote_edit_between_the_listing_and_the_write_is_a_conflict_never_an_overwrite() {
    let server = Server::start().await;
    let replica = server.replica("", 50, Uploads::default());
    let (id, seen) = replica
        .put(new_item("doc.txt", b"v1"), BaseVersion::Absent)
        .await
        .expect("put");

    server.graph.put_file("doc.txt", b"remote edit");
    let now = RemoteVersion(server.graph.etag("doc.txt").expect("etag"));
    let conflict = |base: &RemoteVersion| {
        Err::<(), PutRefused>(PutRefused::Conflict(Conflict {
            item: id.clone(),
            base: BaseVersion::At(base.clone()),
            remote: RemoteSide::Changed(now.clone()),
        }))
    };
    let write = replica
        .put(change_to(&id, b"local edit"), BaseVersion::At(seen.clone()))
        .await;
    assert_eq!(write.map(|_| ()), conflict(&seen));
    assert_eq!(server.graph.file("doc.txt"), Some(b"remote edit".to_vec()));
    // The same for a removal, which names its base too.
    let removal = replica.remove(&id, BaseVersion::At(seen.clone())).await;
    assert_eq!(removal.map(|_| ()), conflict(&seen));
    assert!(
        server.graph.file("doc.txt").is_some(),
        "nothing was deleted"
    );

    // A remote delete is a conflict too, `Deleted`.
    server.graph.delete("doc.txt");
    let late = replica
        .put(change_to(&id, b"late"), BaseVersion::At(now.clone()))
        .await;
    assert_eq!(
        late.map(|_| ()),
        Err(PutRefused::Conflict(Conflict {
            item: id.clone(),
            base: BaseVersion::At(now),
            remote: RemoteSide::Deleted(RemoteVersion(storage_graph::DELETED.into())),
        }))
    );
}

#[tokio::test]
async fn an_expired_anchor_lists_again_and_sends_nothing_a_second_time() {
    let server = Server::start().await;
    for i in 0..12 {
        server
            .graph
            .put_file(&format!("2026/{:02}/IMG_{i}.jpg", i % 3), &[i as u8; 8]);
    }
    let replica = server.replica("", 5, Uploads::default());
    let mut at = Cursor::Start;
    let mut seen = Vec::new();
    loop {
        let page = replica.changes(at).await.expect("page");
        seen.extend(upserts(&page));
        let done = page.more == porter_sync::More::Done;
        at = Cursor::At(page.next.clone());
        if done {
            break;
        }
    }
    assert_eq!(seen.len(), 12, "paged by five, none lost or doubled");
    let anchor = at.clone();

    server.graph.expire_delta_tokens();
    assert_eq!(
        replica.changes(anchor).await.unwrap_err(),
        ReplicaError::AnchorExpired
    );
    // A new process lists from the start: every file again, a fresh anchor, no write at all.
    let fresh = server.replica("", 5, Uploads::default());
    let mut again = 0;
    let mut cursor = Cursor::Start;
    loop {
        let page = fresh.changes(cursor).await.expect("page");
        again += upserts(&page).len();
        if page.more == porter_sync::More::Done {
            break;
        }
        cursor = Cursor::At(page.next);
    }
    assert_eq!(again, 12);
    assert_eq!(writes(&server.graph), vec![], "no upload, no delete");
}

#[tokio::test]
async fn a_download_follows_one_redirect_and_a_range_goes_with_it() {
    let server = Server::start().await;
    server.graph.set_knobs(Knobs {
        redirect_downloads: true,
        ..Knobs::default()
    });
    server.graph.put_file("a.bin", b"0123456789");
    let replica = server.replica("", 50, Uploads::default());
    let page = replica.changes(Cursor::Start).await.expect("listing");
    let Some(Change::Upsert(item)) = page.changes.first() else {
        panic!("{page:?}")
    };
    let span = ByteRange::Span {
        start: Bytes(2),
        len: Bytes(3),
    };
    assert_eq!(
        replica.fetch(&item.id, span).await,
        Ok(Blob(b"234".to_vec()))
    );
    let gets: Vec<(String, u16, Option<String>)> = server
        .graph
        .hits()
        .into_iter()
        .filter(|h| h.target.ends_with("/content") || h.target.starts_with("/dl/"))
        .map(|h| (h.target, h.status, h.range))
        .collect();
    assert_eq!(
        gets,
        vec![
            (
                format!("/v1.0/me/drive/items/{}/content", item.id.0),
                302,
                Some("bytes=2-4".to_owned())
            ),
            (
                format!("/dl/{}", item.id.0),
                206,
                Some("bytes=2-4".to_owned())
            ),
        ]
    );
    // A missing item is Gone, a zero-length span is nothing and asks nothing.
    let sent = server.graph.hits().len();
    let none = ByteRange::Span {
        start: Bytes(0),
        len: Bytes(0),
    };
    assert_eq!(replica.fetch(&item.id, none).await, Ok(Blob(vec![])));
    assert_eq!(server.graph.hits().len(), sent);
    assert_eq!(
        replica
            .fetch(&RemoteId("nope".into()), ByteRange::Whole)
            .await,
        Err(ReplicaError::Gone)
    );
}

#[tokio::test]
async fn throttling_is_a_transient_with_the_wait_the_server_named() {
    let server = Server::start().await;
    let replica = server.replica("", 50, Uploads::default());
    server.graph.throttle(2, 9);
    assert_eq!(
        replica.quota().await,
        Err(ReplicaError::Transient(RetryAfter(9)))
    );
    assert_eq!(
        replica
            .put(new_item("a.jpg", b"x"), BaseVersion::Absent)
            .await,
        Err(PutRefused::Transient(RetryAfter(9)))
    );
    assert!(replica.quota().await.is_ok(), "the throttle ran out");
}

#[tokio::test]
async fn without_the_relays_bearer_every_request_is_refused_and_the_replica_never_adds_one() {
    let server = Server::start().await;
    let bare = GraphReplica::new(
        StreamHttp::new(TcpDial(server.port), StreamLimits::default()),
        &server.base,
        "",
        Clock::fixed(common::NOW),
    );
    assert_eq!(bare.quota().await, Err(ReplicaError::Unauthorized));
    assert_eq!(
        bare.put(new_item("a.jpg", b"x"), BaseVersion::Absent).await,
        Err(PutRefused::Forbidden)
    );
    assert!(
        server
            .graph
            .hits()
            .iter()
            .all(|h| h.authorization.is_none()),
        "the replica itself sends no credential"
    );

    let relayed: Replica = server.replica("", 50, Uploads::default());
    relayed.quota().await.expect("through the relay");
    let want = format!("Bearer {TOKEN}");
    assert!(
        server
            .graph
            .hits()
            .iter()
            .rev()
            .take(1)
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );
}

#[tokio::test]
async fn the_features_say_what_the_replica_does() {
    use porter_core::capability::{Access, Delta, HashKind, Offered, QuotaReport, StorageScope};
    let server = Server::start().await;
    let features = server.replica("", 50, Uploads::default()).features();
    assert_eq!(features.access, Access::ReadWrite);
    assert_eq!(features.delta, Delta::Poll);
    assert_eq!(features.quota, QuotaReport::Reported);
    assert_eq!(features.scope, StorageScope::AppFolder);
    assert_eq!(features.hashes, HashKind::QuickXor);
    assert_eq!(features.ranges, Offered::Present);
    assert_eq!(features.chunked_upload, Offered::Present);
    let _ = WebUrl::parse("http://127.0.0.1:1").expect("url");
}
