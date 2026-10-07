//! porter-sync's contract tests (`porter-sync/tests/contract.rs`), the same cases, run against the
//! reference `MemoryReplica` (so the suite is the reference's) and against `GdriveReplica` over the
//! fake Google's Drive, in two ways it can differ: small files by multipart upload and every file
//! by resumable session. A rule the memory replica passes, the Drive replica passes.

mod common;

use common::{Replica, Server};
use porter_core::capability::{
    Access, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{Bytes, UnixSeconds};
use porter_sync::{
    BaseVersion, Blob, ByteRange, Change, Conflict, Cursor, ItemPath, MemoryReplica, More, PutItem,
    PutRefused, PutTarget, Quota, RemoteId, RemoteSide, Replica as _, ReplicaError, Tombstone,
};
use storage_gdrive::Uploads;

/// A replica under test, its server, and the two knobs the contract needs from a server: an
/// expired feed and a size limit.
trait Harness: Sized {
    type R: porter_sync::Replica;
    async fn start(page: usize, limit: Option<u64>) -> Self;
    fn replica(&self) -> &Self::R;
    /// Every anchor handed out so far is now expired.
    fn expire_anchors(&mut self);
}

struct Memory(MemoryReplica);

impl Harness for Memory {
    type R = MemoryReplica;

    async fn start(page: usize, limit: Option<u64>) -> Self {
        let features = StorageCap {
            access: Access::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Unreported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::Sha256,
            ranges: Offered::Present,
            chunked_upload: Offered::Absent,
        };
        let replica = MemoryReplica::new(features, page, UnixSeconds(1_000));
        Self(match limit {
            Some(total) => replica.with_limit(Bytes(total)),
            None => replica,
        })
    }

    fn replica(&self) -> &MemoryReplica {
        &self.0
    }

    fn expire_anchors(&mut self) {
        self.0.compact();
    }
}

/// How the Drive replica is set up for a run of the suite.
trait Setup {
    fn uploads() -> Uploads;
}

/// Small files in one multipart request (every file the suite writes is small).
struct Simple;
/// Every file through a resumable session.
struct Sessions;

impl Setup for Simple {
    fn uploads() -> Uploads {
        Uploads::default()
    }
}

impl Setup for Sessions {
    fn uploads() -> Uploads {
        Uploads::new(0, 1)
    }
}

struct Drive<S: Setup> {
    server: Server,
    replica: Replica,
    _setup: std::marker::PhantomData<S>,
}

impl<S: Setup> Harness for Drive<S> {
    type R = Replica;

    async fn start(page: usize, limit: Option<u64>) -> Self {
        let server = Server::start().await;
        if let Some(total) = limit {
            server.google.drive_set_limit(total);
        }
        let replica = server.replica("", page, S::uploads());
        Self {
            server,
            replica,
            _setup: std::marker::PhantomData,
        }
    }

    fn replica(&self) -> &Replica {
        &self.replica
    }

    fn expire_anchors(&mut self) {
        // The server dropped its page tokens.
        self.server.google.drive_expire_tokens();
    }
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

async fn head(replica: &impl porter_sync::Replica) -> Cursor {
    let page = replica.changes(Cursor::Start).await.expect("listing");
    Cursor::At(page.next)
}

async fn a_write_appears_once_in_the_feed_after_the_anchor<H: Harness>() {
    let h = H::start(10, None).await;
    let replica = h.replica();
    let before = head(replica).await;
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

async fn a_stale_base_is_a_conflict_value_and_nothing_is_overwritten<H: Harness>() {
    let h = H::start(10, None).await;
    let replica = h.replica();
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

async fn creating_over_an_existing_path_is_a_conflict<H: Harness>() {
    let h = H::start(10, None).await;
    let replica = h.replica();
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

async fn a_removal_leaves_a_tombstone_and_refuses_later_writes_on_the_old_base<H: Harness>() {
    let h = H::start(10, None).await;
    let replica = h.replica();
    let (id, version) = replica
        .put(new_item("a.jpg", b"one"), BaseVersion::Absent)
        .await
        .expect("put");
    let before = head(replica).await;
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

async fn an_expired_anchor_resets_to_a_full_listing_without_tombstones<H: Harness>() {
    let mut h = H::start(10, None).await;
    let old = head(h.replica()).await;
    let (kept, _) = h
        .replica()
        .put(new_item("kept.jpg", b"k"), BaseVersion::Absent)
        .await
        .expect("put");
    let (dropped, v) = h
        .replica()
        .put(new_item("gone.jpg", b"g"), BaseVersion::Absent)
        .await
        .expect("put");
    h.replica()
        .remove(&dropped, BaseVersion::At(v))
        .await
        .expect("remove");
    h.expire_anchors();
    let replica = h.replica();
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

async fn the_feed_pages_until_done<H: Harness>() {
    let h = H::start(2, None).await;
    let replica = h.replica();
    let before = head(replica).await;
    for name in ["a", "b", "c"] {
        replica
            .put(new_item(name, b"x"), BaseVersion::Absent)
            .await
            .expect("put");
    }
    let first = replica.changes(before).await.expect("page 1");
    assert_eq!((first.changes.len(), first.more), (2, More::More));
    let second = replica
        .changes(Cursor::At(first.next))
        .await
        .expect("page 2");
    assert_eq!((second.changes.len(), second.more), (1, More::Done));
    let after = replica
        .changes(Cursor::At(second.next))
        .await
        .expect("after the last page");
    assert_eq!(after.changes, vec![]);
}

async fn a_range_fetch_returns_only_that_span<H: Harness>() {
    let h = H::start(10, None).await;
    let replica = h.replica();
    let (id, _) = replica
        .put(new_item("a.bin", b"0123456789"), BaseVersion::Absent)
        .await
        .expect("put");
    let span = ByteRange::Span {
        start: Bytes(2),
        len: Bytes(3),
    };
    assert_eq!(replica.fetch(&id, span).await, Ok(Blob(b"234".to_vec())));
}

async fn the_quota_counts_live_bytes_and_a_removal_gives_them_back<H: Harness>() {
    let h = H::start(10, None).await;
    let replica = h.replica();
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

async fn a_write_past_the_limit_is_refused_and_a_rewrite_counts_only_the_difference<H: Harness>() {
    let h = H::start(10, Some(10)).await;
    let replica = h.replica();
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

macro_rules! suite {
    ($name:ident: $harness:ty) => {
        mod $name {
            use super::*;
            suite!(@cases $harness;
                a_write_appears_once_in_the_feed_after_the_anchor,
                a_stale_base_is_a_conflict_value_and_nothing_is_overwritten,
                creating_over_an_existing_path_is_a_conflict,
                a_removal_leaves_a_tombstone_and_refuses_later_writes_on_the_old_base,
                an_expired_anchor_resets_to_a_full_listing_without_tombstones,
                the_feed_pages_until_done,
                a_range_fetch_returns_only_that_span,
                the_quota_counts_live_bytes_and_a_removal_gives_them_back,
                a_write_past_the_limit_is_refused_and_a_rewrite_counts_only_the_difference
            );
        }
    };
    (@cases $harness:ty; $($case:ident),+) => {
        $(
            #[tokio::test]
            async fn $case() {
                super::$case::<$harness>().await;
            }
        )+
    };
}

suite!(memory_reference: Memory);
suite!(drive_multipart_upload: Drive<Simple>);
suite!(drive_resumable_session: Drive<Sessions>);
