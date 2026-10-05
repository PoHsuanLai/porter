//! The one trait every backend family implements (File Provider and rclone shape).

use crate::anchor::Cursor;
use crate::change::ChangePage;
use crate::item::{BaseVersion, RemoteId, RemoteVersion};
use crate::quota::Quota;
use crate::refusal::{PutRefused, ReplicaError};
use crate::transfer::{Blob, ByteRange, PutItem};
use porter_core::capability::StorageCap;
use std::future::Future;

/// A dataset's folder in one Storage account. Every operation is resumable and idempotent.
pub trait Replica: Send + Sync {
    /// Changes after `from`, one page at a time.
    fn changes(
        &self,
        from: Cursor,
    ) -> impl Future<Output = Result<ChangePage, ReplicaError>> + Send;

    /// An item's bytes.
    fn fetch(
        &self,
        item: &RemoteId,
        range: ByteRange,
    ) -> impl Future<Output = Result<Blob, ReplicaError>> + Send;

    /// Writes an item if the replica still has `base`.
    fn put(
        &self,
        item: PutItem,
        base: BaseVersion,
    ) -> impl Future<Output = Result<(RemoteId, RemoteVersion), PutRefused>> + Send;

    /// Deletes an item if the replica still has `base`, leaving a tombstone in the feed.
    fn remove(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> impl Future<Output = Result<RemoteVersion, PutRefused>> + Send;

    /// What the account has used and may use (`StorageCap.quota` says whether the backend
    /// reports it; a backend that does not answers `total: None`).
    fn quota(&self) -> impl Future<Output = Result<Quota, ReplicaError>> + Send;

    /// What the backend offers (delta, hashes, ranges, chunked upload).
    fn features(&self) -> StorageCap;
}
