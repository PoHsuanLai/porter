//! What is WebDAV's own in the replica, over the fake server: the headers a write carries, a
//! concurrent remote edit, the fallback from sync-collection to the tree walk, a removed folder,
//! a restarted replica, parents made on the way, and an expired anchor that uploads nothing.

mod common;

use common::{Flavor, Server};
use porter_sync::{
    BaseVersion, Blob, ByteRange, Change, Conflict, Cursor, ItemPath, PutItem, PutRefused,
    PutTarget, RemoteSide, Replica, ReplicaError,
};

fn new_item(path: &str, bytes: &[u8]) -> PutItem {
    PutItem {
        target: PutTarget::New(ItemPath(path.into())),
        content: Blob(bytes.to_vec()),
        hash: None,
    }
}

fn count(server: &Server, method: &str) -> usize {
    server
        .dav
        .hits()
        .iter()
        .filter(|hit| hit.method == method)
        .count()
}

#[tokio::test]
async fn a_write_carries_its_base_as_a_precondition_and_a_remote_edit_is_a_conflict() {
    let server = Server::start(Flavor::Sync).await;
    let replica = server.replica(10);
    let (id, first) = replica
        .put(new_item("a.txt", b"mine"), BaseVersion::Absent)
        .await
        .expect("put");
    // Someone else edits the file in the meantime.
    server.dav.put_file("ds/a.txt", b"theirs");
    let refused = replica
        .put(
            PutItem {
                target: PutTarget::Existing(id.clone()),
                content: Blob(b"mine again".to_vec()),
                hash: None,
            },
            BaseVersion::At(first.clone()),
        )
        .await;
    let Err(PutRefused::Conflict(Conflict { item, base, remote })) = refused else {
        panic!("{refused:?}")
    };
    let RemoteSide::Changed(now) = remote else {
        panic!("{remote:?}")
    };
    assert_eq!((item, base), (id.clone(), BaseVersion::At(first.clone())));
    assert_ne!(now, first);
    assert_eq!(
        replica.fetch(&id, ByteRange::Whole).await,
        Ok(Blob(b"theirs".to_vec()))
    );
    let puts: Vec<_> = server
        .dav
        .hits()
        .into_iter()
        .filter(|hit| hit.method == "PUT")
        .map(|hit| (hit.if_none_match, hit.if_match, hit.status))
        .collect();
    assert_eq!(
        puts,
        [
            (Some("*".to_owned()), None, 201),
            (None, Some(first.0), 412)
        ]
    );
}

#[tokio::test]
async fn a_server_without_sync_collection_is_walked_and_remembered_as_one() {
    let server = Server::start(Flavor::Tree).await;
    server.dav.put_file("ds/a.txt", b"a");
    let replica = server.replica(10);
    let listing = replica.changes(Cursor::Start).await.expect("listing");
    assert_eq!(listing.changes.len(), 1);
    assert!(listing.next.0.starts_with("tree:"), "{:?}", listing.next);
    assert_eq!(count(&server, "REPORT"), 1);
    // A second listing does not ask the REPORT again.
    let listing = replica.changes(Cursor::Start).await.expect("listing");
    assert_eq!(count(&server, "REPORT"), 1);
    // The walk sees an edit, and a deletion as a tombstone.
    server.dav.put_file("ds/a.txt", b"changed");
    server.dav.delete_file("ds/a.txt");
    let page = replica
        .changes(Cursor::At(listing.next))
        .await
        .expect("changes");
    assert!(
        matches!(page.changes.as_slice(), [Change::Tombstone(_)]),
        "{page:?}"
    );
}

#[tokio::test]
async fn a_removed_folder_is_a_tombstone_for_every_file_in_it() {
    let server = Server::start(Flavor::Sync).await;
    server.dav.mkdir("ds/a");
    server.dav.mkdir("ds/a/b");
    server.dav.put_file("ds/a/one.txt", b"1");
    server.dav.put_file("ds/a/b/two.txt", b"2");
    server.dav.put_file("ds/keep.txt", b"k");
    let replica = server.replica(10);
    let listing = replica.changes(Cursor::Start).await.expect("listing");
    assert_eq!(listing.changes.len(), 3);
    assert!(listing.next.0.starts_with("sync:"));
    server.dav.delete_file("ds/a");
    let page = replica
        .changes(Cursor::At(listing.next))
        .await
        .expect("changes");
    let mut gone: Vec<String> = page
        .changes
        .iter()
        .map(|change| match change {
            Change::Tombstone(t) => t.id.0.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    gone.sort();
    assert_eq!(
        gone,
        ["/dav/files/ds/a/b/two.txt", "/dav/files/ds/a/one.txt"]
    );
}

#[tokio::test]
async fn a_restarted_replica_keeps_a_sync_anchor_until_a_removal_needs_what_it_forgot() {
    let server = Server::start(Flavor::Sync).await;
    server.dav.put_file("ds/a.txt", b"a");
    let first = server.replica(10);
    let anchor = first.changes(Cursor::Start).await.expect("listing").next;
    server.dav.put_file("ds/b.txt", b"b");
    let second = server.replica(10);
    let page = second
        .changes(Cursor::At(anchor.clone()))
        .await
        .expect("a token outlives the process");
    assert!(
        matches!(page.changes.as_slice(), [Change::Upsert(_)]),
        "{page:?}"
    );
    server.dav.delete_file("ds/b.txt");
    let third = server.replica(10);
    assert_eq!(
        third.changes(Cursor::At(page.next)).await,
        Err(ReplicaError::AnchorExpired)
    );
}

#[tokio::test]
async fn folders_on_the_way_are_made_once() {
    let server = Server::start(Flavor::Sync).await;
    let replica = server.replica(10);
    for name in ["x/y/1.jpg", "x/y/2.jpg"] {
        replica
            .put(new_item(name, b"."), BaseVersion::Absent)
            .await
            .expect("put");
    }
    assert_eq!(count(&server, "MKCOL"), 2);
    let listing = replica.changes(Cursor::Start).await.expect("listing");
    let paths: Vec<String> = listing
        .changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(item) => Some(item.path.0.clone()),
            Change::Tombstone(_) => None,
        })
        .collect();
    assert_eq!(paths, ["x/y/1.jpg", "x/y/2.jpg"]);
}

#[tokio::test]
async fn an_expired_anchor_in_either_feed_uploads_nothing() {
    for flavor in [Flavor::Sync, Flavor::Tree] {
        let server = Server::start(flavor).await;
        let replica = server.replica(10);
        let old = replica.changes(Cursor::Start).await.expect("listing").next;
        let mut versions = Vec::new();
        for name in ["a", "b"] {
            let (_, version) = replica
                .put(new_item(name, name.as_bytes()), BaseVersion::Absent)
                .await
                .expect("put");
            versions.push(version);
        }
        server.dav.expire_sync_tokens();
        let restarted = server.replica(10);
        assert_eq!(
            restarted.changes(Cursor::At(old)).await,
            Err(ReplicaError::AnchorExpired),
            "{flavor:?}"
        );
        let relist = restarted.changes(Cursor::Start).await.expect("relist");
        let mut again: Vec<_> = relist
            .changes
            .iter()
            .filter_map(|c| match c {
                Change::Upsert(item) => Some(item.version.clone()),
                Change::Tombstone(_) => None,
            })
            .collect();
        again.sort_by(|a, b| a.0.cmp(&b.0));
        versions.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            again, versions,
            "the listing names what is known, at its versions"
        );
        assert_eq!(
            count(&server, "PUT"),
            2,
            "{flavor:?}: no PUT after the expiry"
        );
    }
}
