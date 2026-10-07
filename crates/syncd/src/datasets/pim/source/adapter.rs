//! [`FeedReplica`]: a [`Feed`] as the `Replica` the engine runs over.
//!
//! The feed's typed cursor becomes the anchor text. Content that came with an upsert is kept (in
//! memory, until fetched or until the next page replaces it) so the engine's `fetch` costs no
//! request; a restart between the page and the fetch asks the feed for it. Writes are refused:
//! a mirror is pull-only and the engine never sends one.

use super::{Feed, FeedChange, FeedCursor};
use porter_sync::{
    Anchor, BaseVersion, Blob, ByteRange, Change, ChangePage, Cursor, PutItem, PutRefused, Quota,
    RemoteId, RemoteVersion, Replica, ReplicaError,
};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// The most bytes of content kept between a page and its fetches.
const HELD_MAX: usize = 32 * 1024 * 1024;

#[derive(Debug, Default)]
struct Held {
    bytes: usize,
    items: HashMap<RemoteId, Vec<u8>>,
}

/// A [`Feed`] as a [`Replica`].
#[derive(Debug)]
pub struct FeedReplica<F> {
    feed: F,
    held: Mutex<Held>,
}

impl<F: Feed> FeedReplica<F> {
    /// The replica over `feed`.
    pub fn new(feed: F) -> Self {
        Self {
            feed,
            held: Mutex::default(),
        }
    }

    fn keep(&self, id: &RemoteId, content: Vec<u8>) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.bytes + content.len() > HELD_MAX {
            // Whatever is dropped is asked for again; nothing depends on it.
            held.items.clear();
            held.bytes = 0;
        }
        held.bytes += content.len();
        if let Some(old) = held.items.insert(id.clone(), content) {
            held.bytes -= old.len();
        }
    }

    fn take(&self, id: &RemoteId) -> Option<Vec<u8>> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let content = held.items.remove(id)?;
        held.bytes -= content.len();
        Some(content)
    }
}

fn cut(bytes: Vec<u8>, range: ByteRange) -> Vec<u8> {
    match range {
        ByteRange::Whole => bytes,
        ByteRange::Span { start, len } => {
            let from = usize::try_from(start.0)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            let to = from
                .saturating_add(usize::try_from(len.0).unwrap_or(usize::MAX))
                .min(bytes.len());
            bytes[from..to].to_vec()
        }
    }
}

impl<F: Feed> Replica for FeedReplica<F> {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        let from = match from {
            Cursor::Start => None,
            Cursor::At(Anchor(text)) => {
                Some(F::Cursor::decode(&text).ok_or(ReplicaError::AnchorExpired)?)
            }
        };
        let page = self.feed.changes(from).await?;
        let mut changes = Vec::with_capacity(page.changes.len());
        for change in page.changes {
            changes.push(match change {
                FeedChange::Upsert { item, content } => {
                    if let Some(content) = content {
                        self.keep(&item.id, content);
                    }
                    Change::Upsert(item)
                }
                FeedChange::Delete(tombstone) => Change::Tombstone(tombstone),
            });
        }
        Ok(ChangePage {
            changes,
            next: Anchor(page.next.encode()),
            more: page.more,
        })
    }

    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let bytes = match self.take(item) {
            Some(bytes) => bytes,
            None => self.feed.fetch(item).await?,
        };
        Ok(Blob(cut(bytes, range)))
    }

    async fn put(
        &self,
        _item: PutItem,
        _base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        Err(PutRefused::Forbidden)
    }

    async fn remove(
        &self,
        _item: &RemoteId,
        _base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        Err(PutRefused::Forbidden)
    }

    async fn quota(&self) -> Result<Quota, ReplicaError> {
        self.feed.quota().await
    }

    fn features(&self) -> porter_core::capability::StorageCap {
        self.feed.features()
    }
}
