//! The sync contract (design/31 §6.1), driven through the reference replica: anchors, base
//! versions, conflicts as values, tombstones, and cursor reset.

use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{Bytes, UnixSeconds};
use porter_sync::{
    BaseVersion, Blob, ByteRange, Change, ChangePage, Conflict, Cursor, ItemPath, MemoryReplica,
    More, PutItem, PutRefused, PutTarget, Quota, RemoteId, RemoteSide, Replica, ReplicaError,
    Tombstone,
};

fn replica(page: usize) -> MemoryReplica {
    let features = StorageCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        quota: QuotaReport::Unreported,
        scope: StorageScope::AppFolder,
        hashes: HashKind::Sha256,
        ranges: Offered::Present,
        chunked_upload: Offered::Absent,
    };
    MemoryReplica::new(features, page, UnixSeconds(1_000))
}

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

async fn head(replica: &MemoryReplica) -> Cursor {
    let page = replica.changes(Cursor::Start).await.expect("listing");
    Cursor::At(page.next)
}

#[tokio::test]
async fn a_write_appears_once_in_the_feed_after_the_anchor() {
    let replica = replica(10);
    let before = head(&replica).await;
    let (id, version) = replica
        .put(new_item("a.jpg", b"one"), BaseVersion::Absent)
        .await
        .expect("put");
    let page = replica.changes(before).await.expect("changes");
    let [Change::Upsert(item)] = page.changes.as_slice() else {
        panic!("{page:?}")
    };
    assert_eq!(
        (&item.id, &item.version, page.more),
        (&id, &version, More::Done)
    );
    let again = replica
        .changes(Cursor::At(page.next))
        .await
        .expect("changes");
    assert_eq!(again.changes, vec![]);
}

#[tokio::test]
async fn a_stale_base_is_a_conflict_value_and_nothing_is_overwritten() {
    let replica = replica(10);
    let (id, first) = replica
        .put(new_item("a.jpg", b"one"), BaseVersion::Absent)
        .await
        .expect("put");
    let (_, second) = replica
        .put(change_to(&id, b"two"), BaseVersion::At(first.clone()))
        .await
        .expect("put");
    let refused = replica
        .put(change_to(&id, b"three"), BaseVersion::At(first.clone()))
        .await;
    let expected = Conflict {
        item: id.clone(),
        base: BaseVersion::At(first),
        remote: RemoteSide::Changed(second),
    };
    assert_eq!(refused, Err(PutRefused::Conflict(expected)));
    assert_eq!(
        replica.fetch(&id, ByteRange::Whole).await,
        Ok(Blob(b"two".to_vec()))
    );
}

#[tokio::test]
async fn creating_over_an_existing_path_is_a_conflict() {
    let replica = replica(10);
    let (id, version) = replica
        .put(new_item("a.jpg", b"one"), BaseVersion::Absent)
        .await
        .expect("put");
    let refused = replica
        .put(new_item("a.jpg", b"other"), BaseVersion::Absent)
        .await;
    let expected = Conflict {
        item: id.clone(),
        base: BaseVersion::Absent,
        remote: RemoteSide::Exists(id, version),
    };
    assert_eq!(refused, Err(PutRefused::Conflict(expected)));
}

#[tokio::test]
async fn a_removal_leaves_a_tombstone_and_refuses_later_writes_on_the_old_base() {
    let replica = replica(10);
    let (id, version) = replica
        .put(new_item("a.jpg", b"one"), BaseVersion::Absent)
        .await
        .expect("put");
    let before = head(&replica).await;
    let gone = replica
        .remove(&id, BaseVersion::At(version.clone()))
        .await
        .expect("remove");
    let page = replica.changes(before).await.expect("changes");
    let tombstone = Tombstone {
        id: id.clone(),
        version: gone.clone(),
        deleted_at: UnixSeconds(1_000),
    };
    assert_eq!(page.changes, vec![Change::Tombstone(tombstone)]);
    let refused = replica
        .put(change_to(&id, b"late"), BaseVersion::At(version.clone()))
        .await;
    let expected = Conflict {
        item: id.clone(),
        base: BaseVersion::At(version),
        remote: RemoteSide::Deleted(gone),
    };
    assert_eq!(refused, Err(PutRefused::Conflict(expected)));
    assert_eq!(
        replica.fetch(&id, ByteRange::Whole).await,
        Err(ReplicaError::Gone)
    );
}

#[tokio::test]
async fn an_expired_anchor_resets_to_a_full_listing_without_tombstones() {
    let replica = replica(10);
    let old = head(&replica).await;
    let (kept, _) = replica
        .put(new_item("kept.jpg", b"k"), BaseVersion::Absent)
        .await
        .expect("put");
    let (dropped, v) = replica
        .put(new_item("gone.jpg", b"g"), BaseVersion::Absent)
        .await
        .expect("put");
    replica
        .remove(&dropped, BaseVersion::At(v))
        .await
        .expect("remove");
    replica.compact();
    assert_eq!(replica.changes(old).await, Err(ReplicaError::AnchorExpired));
    let listing = replica.changes(Cursor::Start).await.expect("listing");
    let ids: Vec<&RemoteId> = listing
        .changes
        .iter()
        .map(|change| match change {
            Change::Upsert(item) => &item.id,
            Change::Tombstone(t) => panic!("listing carries a tombstone: {t:?}"),
        })
        .collect();
    assert_eq!(ids, vec![&kept]);
    let fresh = replica
        .changes(Cursor::At(listing.next))
        .await
        .expect("fresh anchor works");
    assert_eq!(fresh.changes, vec![]);
}

#[tokio::test]
async fn the_feed_pages_until_done() {
    let replica = replica(2);
    let before = head(&replica).await;
    for name in ["a", "b", "c"] {
        replica
            .put(new_item(name, b"x"), BaseVersion::Absent)
            .await
            .expect("put");
    }
    let first: ChangePage = replica.changes(before).await.expect("page 1");
    assert_eq!((first.changes.len(), first.more), (2, More::More));
    let second = replica
        .changes(Cursor::At(first.next))
        .await
        .expect("page 2");
    assert_eq!((second.changes.len(), second.more), (1, More::Done));
}

#[tokio::test]
async fn a_range_fetch_returns_only_that_span() {
    let replica = replica(10);
    let (id, _) = replica
        .put(new_item("a.bin", b"0123456789"), BaseVersion::Absent)
        .await
        .expect("put");
    let span = porter_sync::ByteRange::Span {
        start: porter_core::Bytes(2),
        len: porter_core::Bytes(3),
    };
    assert_eq!(replica.fetch(&id, span).await, Ok(Blob(b"234".to_vec())));
}

#[tokio::test]
async fn the_quota_counts_live_bytes_and_a_removal_gives_them_back() {
    let replica = replica(10);
    assert_eq!(
        replica.quota().await,
        Ok(Quota {
            used: Bytes(0),
            total: None
        })
    );
    let (id, version) = replica
        .put(new_item("a.jpg", b"12345"), BaseVersion::Absent)
        .await
        .expect("put");
    replica
        .put(new_item("b.jpg", b"123"), BaseVersion::Absent)
        .await
        .expect("put");
    assert_eq!(replica.quota().await.map(|q| q.used), Ok(Bytes(8)));
    replica
        .remove(&id, BaseVersion::At(version))
        .await
        .expect("remove");
    assert_eq!(replica.quota().await.map(|q| q.used), Ok(Bytes(3)));
}

#[tokio::test]
async fn a_write_past_the_limit_is_refused_and_a_rewrite_counts_only_the_difference() {
    let replica = replica(10).with_limit(Bytes(10));
    let (id, version) = replica
        .put(new_item("a.jpg", b"123456"), BaseVersion::Absent)
        .await
        .expect("put");
    assert_eq!(
        replica.quota().await,
        Ok(Quota {
            used: Bytes(6),
            total: Some(Bytes(10))
        })
    );
    assert_eq!(
        replica
            .put(new_item("b.jpg", b"12345"), BaseVersion::Absent)
            .await,
        Err(PutRefused::Quota)
    );
    // Replacing the 6 bytes with 9 fits: 9 <= 10, though 6 + 9 would not.
    replica
        .put(change_to(&id, b"123456789"), BaseVersion::At(version))
        .await
        .expect("rewrite fits");
    assert_eq!(replica.quota().await.map(|q| q.used), Ok(Bytes(9)));
}
