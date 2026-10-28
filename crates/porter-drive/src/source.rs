//! Where a drive's bytes come from: a span of one item at a time, and an online document's
//! copy as a file.

use porter_capability::HashKind;
use porter_ids::Bytes;
use porter_sync::{ByteRange, ExportError, RemoteId, Replica, ReplicaError};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Why the bytes did not come.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
            // anchor expired, unauthorized, gone, needs consent, and a failure a later
            // porter-sync adds
            _ => SourceError::Refused,
        }
    }
}

/// Why an online document's copy was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CopyError {
    /// The drive gives no file of it.
    #[error("no file format")]
    NoFormat,
    /// It is larger than the drive gives as a file.
    #[error("too large")]
    TooLarge,
    /// The drive could not be reached, or refused.
    #[error("the copy failed")]
    Failed,
}

impl From<ExportError> for CopyError {
    fn from(error: ExportError) -> Self {
        match error {
            ExportError::Unsupported => CopyError::NoFormat,
            ExportError::TooLarge => CopyError::TooLarge,
            // a read that failed, and a failure a later porter-sync adds
            _ => CopyError::Failed,
        }
    }
}

/// An export under way.
pub type Exporting<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, CopyError>> + Send + 'a>>;

/// A fetch under way.
pub type Fetching<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, SourceError>> + Send + 'a>>;

/// The bytes of a drive's items.
pub trait Source: Send + Sync {
    /// `len` bytes of `id` from `start` (fewer at its end).
    fn fetch<'a>(&'a self, id: &'a RemoteId, start: u64, len: u64) -> Fetching<'a>;

    /// The kind of the hashes the listing has for this drive's files ([`crate::Entry::hash`]),
    /// which a whole file is checked against. This default says none.
    fn hashes(&self) -> HashKind {
        HashKind::None
    }

    /// The online document `id` as a file of `mime`. A drive that gives no copies refuses
    /// (`NoFormat`), as this default does.
    fn export<'a>(&'a self, id: &'a RemoteId, mime: &'a str) -> Exporting<'a> {
        let _ = (id, mime);
        Box::pin(std::future::ready(Err(CopyError::NoFormat)))
    }
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

    fn hashes(&self) -> HashKind {
        self.0.features().hashes
    }

    fn export<'a>(&'a self, id: &'a RemoteId, mime: &'a str) -> Exporting<'a> {
        Box::pin(async move {
            self.0
                .export(id, mime)
                .await
                .map(|blob| blob.0)
                .map_err(CopyError::from)
        })
    }
}
