//! The reference replica: the contract's semantics in memory, for the contract tests and for
//! syncd's tests. Anchors are `seq-<n>` positions in its log; `compact` expires them all.

use crate::anchor::{Anchor, Cursor};
use crate::change::{Change, ChangePage, More, Tombstone};
use crate::item::{BaseVersion, RemoteId, RemoteItem, RemoteVersion};
use crate::refusal::{Conflict, PutRefused, RemoteSide, ReplicaError};
use crate::replica::Replica;
use crate::transfer::{Blob, ByteRange, PutItem, PutTarget};
use porter_core::capability::StorageCap;
use porter_core::{Bytes, UnixSeconds};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

/// A replica held in memory.
#[derive(Debug)]
pub struct MemoryReplica {
    features: StorageCap,
    page: usize,
    now: UnixSeconds,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    head: u64,
    compacted_through: u64,
    log: Vec<(u64, Change)>,
    live: BTreeMap<RemoteId, (RemoteItem, Blob)>,
    deleted: BTreeMap<RemoteId, RemoteVersion>,
}

impl MemoryReplica {
    /// An empty replica offering `features`, paging its feed `page` changes at a time, dating
    /// tombstones `now`.
    pub fn new(features: StorageCap, page: usize, now: UnixSeconds) -> Self {
        Self {
            features,
            page: page.max(1),
            now,
            state: Mutex::default(),
        }
    }

    /// Drops the log, as a server does when a cursor ages out: every anchor handed out so far
    /// is now `AnchorExpired`.
    pub fn compact(&self) {
        let mut state = self.state();
        state.compacted_through = state.head;
        state.log.clear();
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while the lock is held already failed the test that caused it.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn anchor(seq: u64) -> Anchor {
    Anchor(format!("seq-{seq}"))
}

fn seq_of(anchor: &Anchor) -> Option<u64> {
    anchor.0.strip_prefix("seq-")?.parse().ok()
}

impl State {
    fn bump(&mut self) -> (u64, RemoteVersion) {
        self.head += 1;
        (self.head, RemoteVersion(format!("v{}", self.head)))
    }

    /// `Ok` when `base` is the replica's version of `id`, else the conflict.
    fn check(&self, id: &RemoteId, base: &BaseVersion) -> Result<(), PutRefused> {
        let conflict = |remote| Conflict {
            item: id.clone(),
            base: base.clone(),
            remote,
        };
        match (self.live.get(id), self.deleted.get(id), base) {
            (Some((item, _)), _, BaseVersion::At(seen)) if item.version == *seen => Ok(()),
            (Some((item, _)), _, _) => Err(PutRefused::Conflict(conflict(RemoteSide::Changed(
                item.version.clone(),
            )))),
            (None, Some(gone), _) => Err(PutRefused::Conflict(conflict(RemoteSide::Deleted(
                gone.clone(),
            )))),
            (None, None, _) => Err(PutRefused::Forbidden),
        }
    }

    fn page_after(&self, from: u64, size: usize) -> ChangePage {
        let pending: Vec<&(u64, Change)> = self.log.iter().filter(|(seq, _)| *seq > from).collect();
        let taken = &pending[..pending.len().min(size)];
        let next = taken.last().map_or(from, |(seq, _)| *seq);
        ChangePage {
            changes: taken.iter().map(|(_, change)| change.clone()).collect(),
            next: anchor(next),
            more: if pending.len() > taken.len() {
                More::More
            } else {
                More::Done
            },
        }
    }
}

impl Replica for MemoryReplica {
    async fn changes(&self, from: Cursor) -> Result<ChangePage, ReplicaError> {
        let state = self.state();
        match from {
            Cursor::Start => Ok(ChangePage {
                changes: state
                    .live
                    .values()
                    .map(|(item, _)| Change::Upsert(item.clone()))
                    .collect(),
                next: anchor(state.head),
                more: More::Done,
            }),
            Cursor::At(at) => match seq_of(&at) {
                Some(seq) if seq >= state.compacted_through && seq <= state.head => {
                    Ok(state.page_after(seq, self.page))
                }
                _ => Err(ReplicaError::AnchorExpired),
            },
        }
    }

    async fn fetch(&self, item: &RemoteId, range: ByteRange) -> Result<Blob, ReplicaError> {
        let state = self.state();
        let (_, Blob(bytes)) = state.live.get(item).ok_or(ReplicaError::Gone)?;
        let slice = match range {
            ByteRange::Whole => bytes.as_slice(),
            ByteRange::Span {
                start: Bytes(start),
                len: Bytes(len),
            } => {
                let start = usize::try_from(start)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                let end = start
                    .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                    .min(bytes.len());
                &bytes[start..end]
            }
        };
        Ok(Blob(slice.to_vec()))
    }

    async fn put(
        &self,
        item: PutItem,
        base: BaseVersion,
    ) -> Result<(RemoteId, RemoteVersion), PutRefused> {
        let mut state = self.state();
        let (id, path) = match item.target {
            PutTarget::Existing(id) => {
                state.check(&id, &base)?;
                let path = state.live.get(&id).map(|(live, _)| live.path.clone());
                (id, path.ok_or(PutRefused::Forbidden)?)
            }
            PutTarget::New(path) => {
                let taken = state.live.values().find(|(live, _)| live.path == path);
                if let Some((live, _)) = taken {
                    let remote = RemoteSide::Exists(live.id.clone(), live.version.clone());
                    return Err(PutRefused::Conflict(Conflict {
                        item: live.id.clone(),
                        base,
                        remote,
                    }));
                }
                (RemoteId(format!("id-{}", state.head + 1)), path)
            }
        };
        let (seq, version) = state.bump();
        let size = Bytes(item.content.0.len() as u64);
        let stored = RemoteItem {
            id: id.clone(),
            version: version.clone(),
            path,
            size,
            hash: item.hash,
        };
        state.log.push((seq, Change::Upsert(stored.clone())));
        state.deleted.remove(&id);
        state.live.insert(id.clone(), (stored, item.content));
        Ok((id, version))
    }

    async fn remove(
        &self,
        item: &RemoteId,
        base: BaseVersion,
    ) -> Result<RemoteVersion, PutRefused> {
        let mut state = self.state();
        state.check(item, &base)?;
        let (seq, version) = state.bump();
        let tombstone = Tombstone {
            id: item.clone(),
            version: version.clone(),
            deleted_at: self.now,
        };
        state.log.push((seq, Change::Tombstone(tombstone)));
        state.live.remove(item);
        state.deleted.insert(item.clone(), version.clone());
        Ok(version)
    }

    fn features(&self) -> StorageCap {
        self.features.clone()
    }
}
