//! Where a drive's bytes come from: a span of one item at a time.

use porter_core::Bytes;
use porter_sync::{ByteRange, RemoteId, Replica, ReplicaError};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Why the bytes did not come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// The drive could not be reached (no network, or it asked to wait).
    #[error("the drive could not be reached")]
    Offline,
    /// The drive refused (the item is gone, the account must sign in again or has not given
    /// access).
    #[error("the drive refused")]
    Refused,
}

impl From<ReplicaError> for SourceError {
    fn from(error: ReplicaError) -> Self {
        match error {
            ReplicaError::Transient(_) => SourceError::Offline,
            ReplicaError::AnchorExpired
            | ReplicaError::Unauthorized
            | ReplicaError::Gone
            | ReplicaError::NeedsConsent => SourceError::Refused,
        }
    }
}

/// A fetch under way.
pub type Fetching<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, SourceError>> + Send + 'a>>;

/// The bytes of a drive's items.
pub trait Source: Send + Sync {
    /// `len` bytes of `id` from `start` (fewer at its end).
    fn fetch<'a>(&'a self, id: &'a RemoteId, start: u64, len: u64) -> Fetching<'a>;
}

/// A [`Source`] over a porter-sync replica's `fetch`, a span at a time. The replica is shared,
/// so its owner asks it for the quota too.
#[derive(Debug)]
pub struct ReplicaSource<R>(pub Arc<R>);

impl<R: Replica> Source for ReplicaSource<R> {
    fn fetch<'a>(&'a self, id: &'a RemoteId, start: u64, len: u64) -> Fetching<'a> {
        Box::pin(async move {
            let range = ByteRange::Span {
                start: Bytes(start),
                len: Bytes(len),
            };
            self.0
                .fetch(id, range)
                .await
                .map(|blob| blob.0)
                .map_err(SourceError::from)
        })
    }
}
