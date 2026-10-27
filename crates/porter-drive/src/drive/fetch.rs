//! Fetching: one transfer per item being fetched, filling its content file chunk by chunk from
//! the start, and a read that waits for the chunk it needs (asked for before the rest).

use super::{Drive, Failure, Inner, NodeId, held};
use crate::Reason;
use crate::hash;
use crate::index::Row;
use crate::listing::Entry;
use crate::online;
use crate::ranges::Ranges;
use crate::source::SourceError;
use porter_sync::{ContentHash, ItemKind, RemoteId};
use std::collections::HashMap;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Notify, watch};

/// Why a read gave no bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    /// The drive could not be reached: the file downloads when it can be.
    #[error("the drive could not be reached")]
    Offline,
    /// The drive refused to send it.
    #[error("the drive refused to send it")]
    Refused,
    /// It is not there (any more).
    #[error("it is not there")]
    Gone,
    /// It is a folder.
    #[error("it is a folder")]
    Folder,
    /// The cache could not be read or written.
    #[error("the cache could not be used")]
    Cache,
    /// It changed on the drive while it downloaded: the bytes were dropped, and it downloads
    /// again when next asked.
    #[error("it changed on the drive while it downloaded")]
    Changed,
    /// The drive stopped sending it (a chunk took longer than its deadline).
    #[error("the drive stopped sending it")]
    Stalled,
}

impl From<SourceError> for ReadError {
    fn from(error: SourceError) -> Self {
        match error {
            SourceError::Offline => ReadError::Offline,
            SourceError::Refused => ReadError::Refused,
        }
    }
}

/// One item being fetched.
pub(super) struct Transfer {
    pub(super) id: RemoteId,
    pub(super) version: String,
    pub(super) size: u64,
    /// The drive's hash of this version, when the listing has one: the whole file is checked
    /// against it.
    hash: Option<ContentHash>,
    file: PathBuf,
    pub(super) state: Mutex<TransferState>,
    /// Bumped whenever the state moves, for readers to wait on.
    moved: watch::Sender<u64>,
    /// A reader is waiting: a transfer queued behind others starts now.
    promoted: Notify,
}

pub(super) struct TransferState {
    pub(super) present: Ranges,
    /// Chunk starts readers wait for, the next to fetch last.
    wanted: Vec<u64>,
    failure: Option<ReadError>,
    pub(super) cancelled: bool,
    cursor: u64,
}

impl Transfer {
    pub(super) fn moved_bytes(&self) -> u64 {
        held(&self.state).present.bytes()
    }

    pub(super) fn cancel(&self) {
        held(&self.state).cancelled = true;
        self.moved.send_modify(|n| *n += 1);
    }

    fn cancelled(&self) -> bool {
        held(&self.state).cancelled
    }

    fn fail(&self, error: ReadError) {
        held(&self.state).failure = Some(error);
        self.moved.send_modify(|n| *n += 1);
    }
}

fn read_at(path: &PathBuf, offset: u64, len: u64) -> Result<Vec<u8>, ReadError> {
    let file = std::fs::File::open(path).map_err(|_| ReadError::Cache)?;
    let mut buffer = vec![0; usize::try_from(len).map_err(|_| ReadError::Cache)?];
    file.read_exact_at(&mut buffer, offset)
        .map_err(|_| ReadError::Cache)?;
    Ok(buffer)
}

impl Inner {
    /// The transfer that fetches `entry`'s current version, started when none runs; `None` when
    /// it is all here already. `background` waits its turn among the downloads nobody waits on.
    pub(super) fn ensure(
        self: &Arc<Self>,
        entry: &Entry,
        background: bool,
    ) -> Result<Option<Arc<Transfer>>, ReadError> {
        if !entry.kind.is_file() {
            return Ok(None);
        }
        let version = entry.version.0.clone();
        let present = {
            let mut index = held(&self.index);
            let clock = index.clock;
            match index.items.get(&entry.id.0) {
                Some(row) if row.version == version && row.whole() => return Ok(None),
                Some(row) if row.version == version && row.size == entry.size.0 => {
                    row.present.clone()
                }
                stale => {
                    if let Some(old) = stale {
                        let _ = std::fs::remove_file(self.content(&entry.id, &old.version));
                    }
                    index.items.insert(
                        entry.id.0.clone(),
                        Row {
                            version: version.clone(),
                            size: entry.size.0,
                            present: Ranges::default(),
                            opened: clock,
                            checked: false,
                        },
                    );
                    Ranges::default()
                }
            }
        };
        let mut transfers = held(&self.transfers);
        if let Some(running) = transfers.get(&entry.id)
            && running.version == version
            && !running.cancelled()
        {
            return Ok(Some(Arc::clone(running)));
        }
        let file = self.content(&entry.id, &version);
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&file)
            .and_then(|f| f.set_len(entry.size.0))
            .map_err(|_| ReadError::Cache)?;
        let (moved, _) = watch::channel(0);
        let transfer = Arc::new(Transfer {
            id: entry.id.clone(),
            version,
            size: entry.size.0,
            hash: entry.hash.clone(),
            file,
            state: Mutex::new(TransferState {
                present,
                wanted: Vec::new(),
                failure: None,
                cancelled: false,
                cursor: 0,
            }),
            moved,
            promoted: Notify::new(),
        });
        if let Some(old) = transfers.insert(entry.id.clone(), Arc::clone(&transfer)) {
            old.cancel();
        }
        drop(transfers);
        held(&self.errors).remove(&entry.id);
        self.touched();
        let weak = Arc::downgrade(self);
        self.runtime
            .spawn(run(weak, Arc::clone(&transfer), background));
        self.emit(vec![NodeId::Remote(entry.id.clone())]);
        Ok(Some(transfer))
    }

    /// The transfer of `id` running now, if any.
    pub(super) fn transfer(&self, id: &RemoteId) -> Option<Arc<Transfer>> {
        held(&self.transfers).get(id).cloned()
    }

    /// Whether `transfer` is the one registered for its item.
    fn current_in(transfers: &HashMap<RemoteId, Arc<Transfer>>, transfer: &Arc<Transfer>) -> bool {
        transfers
            .get(&transfer.id)
            .is_some_and(|running| Arc::ptr_eq(running, transfer))
    }

    /// Ends `transfer` (done, failed or cancelled): it leaves the running ones.
    fn retire(&self, transfer: &Arc<Transfer>) {
        let mut transfers = held(&self.transfers);
        if Self::current_in(&transfers, transfer) {
            transfers.remove(&transfer.id);
        }
    }

    /// Ends `transfer` with `error`: when it is still its item's transfer, the item says why
    /// (a transfer cancelled or replaced says nothing of the one after it).
    fn failed(&self, transfer: &Arc<Transfer>, error: ReadError) {
        transfer.fail(error);
        let mut transfers = held(&self.transfers);
        if !Self::current_in(&transfers, transfer) {
            return;
        }
        transfers.remove(&transfer.id);
        if error == ReadError::Offline {
            self.offline.store(true, Ordering::SeqCst);
        }
        held(&self.errors).insert(
            transfer.id.clone(),
            Failure {
                reason: Reason::Offline,
                transient: matches!(error, ReadError::Offline | ReadError::Stalled),
            },
        );
    }

    /// Notes `present` of `transfer` in the index (`checked`: it is all there and checked), if
    /// it is still its item's transfer and the row names its version; whether it was noted.
    fn note(&self, transfer: &Arc<Transfer>, present: &Ranges, checked: bool) -> bool {
        let mut index = held(&self.index);
        if !Self::current_in(&held(&self.transfers), transfer) {
            return false;
        }
        let Some(row) = index.items.get_mut(&transfer.id.0) else {
            return false;
        };
        if row.version != transfer.version {
            return false;
        }
        row.present = present.clone();
        row.checked = checked;
        drop(index);
        self.touched();
        true
    }

    /// Drops what `transfer` fetched, which is not the drive's version it was asked for: the
    /// item is stale and fetched again from the start when next asked.
    fn stale(&self, transfer: &Arc<Transfer>) {
        let mut index = held(&self.index);
        let mut transfers = held(&self.transfers);
        if !Self::current_in(&transfers, transfer) {
            return;
        }
        transfers.remove(&transfer.id);
        drop(transfers);
        if index
            .items
            .get(&transfer.id.0)
            .is_some_and(|row| row.version == transfer.version)
        {
            index.items.remove(&transfer.id.0);
            let _ = std::fs::remove_file(&transfer.file);
        }
        drop(index);
        self.touched();
    }

    /// The drive was reached again: what failed for want of it says so no more.
    pub(super) fn came_online(&self) {
        let cleared: Vec<NodeId> = {
            let mut errors = held(&self.errors);
            let ids: Vec<RemoteId> = errors
                .iter()
                .filter(|(_, failure)| failure.transient)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &ids {
                errors.remove(id);
            }
            ids.into_iter().map(NodeId::Remote).collect()
        };
        self.wake.notify_one();
        self.emit(cleared);
    }
}

/// Fills `transfer`'s file: the chunks readers wait for first, then the rest from the start;
/// then checks it.
async fn run(weak: Weak<Inner>, transfer: Arc<Transfer>, background: bool) {
    let _permit = match background {
        true => {
            let Some(inner) = weak.upgrade() else { return };
            let semaphore = Arc::clone(&inner.background);
            drop(inner);
            tokio::select! {
                permit = semaphore.acquire_owned() => permit.ok(),
                () = transfer.promoted.notified() => None,
            }
        }
        false => None,
    };
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let chunk = inner.chunk;
        let next = {
            let mut state = held(&transfer.state);
            if state.cancelled {
                drop(state);
                inner.retire(&transfer);
                return;
            }
            let mut next = None;
            while let Some(start) = state.wanted.pop() {
                if !state
                    .present
                    .covers(start, (start + chunk).min(transfer.size))
                {
                    next = Some(start);
                    break;
                }
            }
            next.or_else(|| {
                state
                    .present
                    .first_gap(state.cursor, transfer.size)
                    .or_else(|| state.present.first_gap(0, transfer.size))
            })
        };
        let Some(start) = next else {
            drop(inner);
            finish(weak, transfer).await;
            return;
        };
        let len = chunk.min(transfer.size - start);
        let source = Arc::clone(&inner.source);
        let deadline = inner.deadline;
        drop(inner);
        let fetched =
            match tokio::time::timeout(deadline, source.fetch(&transfer.id, start, len)).await {
                Ok(Ok(bytes)) if bytes.is_empty() => Err(ReadError::Refused),
                Ok(Ok(bytes)) => Ok(bytes),
                Ok(Err(error)) => Err(ReadError::from(error)),
                Err(_) => Err(ReadError::Stalled),
            };
        let Some(inner) = weak.upgrade() else { return };
        if transfer.cancelled() {
            inner.retire(&transfer);
            return;
        }
        match fetched {
            Ok(bytes) => {
                let bytes = &bytes[..bytes.len().min(usize::try_from(len).unwrap_or(usize::MAX))];
                let wrote = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&transfer.file)
                    .and_then(|file| file.write_all_at(bytes, start));
                if wrote.is_err() {
                    // The file went (the download was removed): nothing more to do.
                    transfer.cancel();
                    inner.retire(&transfer);
                    return;
                }
                inner.wrote(&transfer.file);
                if inner.offline.swap(false, Ordering::SeqCst) {
                    inner.came_online();
                }
                let n = bytes.len() as u64;
                let present = {
                    let mut state = held(&transfer.state);
                    if state.cancelled {
                        continue;
                    }
                    state.present.insert(start, start + n);
                    state.cursor = start + n;
                    state.present.clone()
                };
                inner.note(&transfer, &present, false);
                transfer.moved.send_modify(|n| *n += 1);
                inner.emit(vec![NodeId::Remote(transfer.id.clone())]);
            }
            Err(error) => {
                inner.failed(&transfer, error);
                inner.emit(vec![NodeId::Remote(transfer.id.clone())]);
                return;
            }
        }
    }
}

/// Every byte of `transfer` is here: it is checked against the drive's hash of its version
/// (or, with none this crate computes, against the listing's version now), then noted whole,
/// or dropped as stale.
async fn finish(weak: Weak<Inner>, transfer: Arc<Transfer>) {
    let Some(inner) = weak.upgrade() else { return };
    let kind = inner.source.hashes();
    let expected = transfer.hash.clone().filter(|_| hash::computes(kind));
    let same = match expected {
        Some(expected) => {
            drop(inner);
            let file = transfer.file.clone();
            let got = tokio::task::spawn_blocking(move || hash::of_file(kind, &file)).await;
            let Some(again) = weak.upgrade() else { return };
            match got {
                Ok(Ok(Some(got))) => Some((again, got.eq_ignore_ascii_case(&expected.0))),
                // The file went (the download was removed), or could not be read.
                _ => {
                    again.retire(&transfer);
                    None
                }
            }
        }
        None => {
            let same = inner
                .entry_open(&transfer.id)
                .ok()
                .flatten()
                .is_some_and(|entry| entry.version.0 == transfer.version);
            Some((inner, same))
        }
    };
    let Some((inner, same)) = same else {
        transfer.fail(ReadError::Cache);
        return;
    };
    match same {
        true => {
            inner.note(&transfer, &Ranges::whole(transfer.size), true);
            inner.retire(&transfer);
            transfer.moved.send_modify(|n| *n += 1);
        }
        false => {
            inner.stale(&transfer);
            transfer.fail(ReadError::Changed);
        }
    }
    inner.emit(vec![NodeId::Remote(transfer.id.clone())]);
    inner.evict();
}

impl Inner {
    /// `len` bytes from `offset` of the bytes `id` has here, read under their lock; `None` when
    /// they went meanwhile (sent: the cache has them now).
    fn read_dirty(&self, id: &NodeId, offset: u64, len: u64) -> Option<Result<Vec<u8>, ReadError>> {
        let n = held(&self.index)
            .edits
            .get(id)
            .and_then(|edit| edit.dirty.as_ref())
            .map(|dirty| dirty.file)?;
        let lock = self.file_lock(n);
        let _bytes = held(&lock);
        let size = held(&self.index)
            .edits
            .get(id)
            .and_then(|edit| edit.dirty.as_ref())
            .filter(|dirty| dirty.file == n)
            .map(|dirty| dirty.size)?;
        let end = offset.saturating_add(len).min(size);
        Some(match offset < end {
            true => read_at(&self.local_file(n), offset, end - offset),
            false => Ok(Vec::new()),
        })
    }
}

impl Drive {
    /// Opens `id` for reading: a file not here starts downloading from its start, and it is not
    /// evicted until [`Drive::release`]. Its last opened time is now.
    ///
    /// # Errors
    /// It is a folder, it is not there, or its download could not start.
    pub fn open_file(&self, id: &NodeId) -> Result<(), ReadError> {
        let inner = &self.inner;
        let id = &inner.canonical(id);
        if let Some(edit) = inner.edit(id) {
            if edit.folder {
                return Err(ReadError::Folder);
            }
            if edit.dirty.is_some() {
                // Its bytes are the ones written here.
                *held(&inner.open).entry(id.clone()).or_default() += 1;
                return Ok(());
            }
        }
        if let NodeId::Remote(remote) = id {
            let entry = inner
                .entry(remote)
                .map_err(|_| ReadError::Cache)?
                .ok_or(ReadError::Gone)?;
            match entry.kind {
                ItemKind::Folder => return Err(ReadError::Folder),
                ItemKind::File => {
                    {
                        let mut index = held(&inner.index);
                        index.clock += 1;
                        let clock = index.clock;
                        if let Some(row) = index.items.get_mut(&remote.0) {
                            row.opened = clock;
                        }
                    }
                    inner.touched();
                    *held(&inner.open).entry(id.clone()).or_default() += 1;
                    if let Err(error) = inner.ensure(&entry, false) {
                        self.release(id);
                        return Err(error);
                    }
                }
                _ => {}
            }
        } else if *id == NodeId::Top {
            return Err(ReadError::Folder);
        }
        Ok(())
    }

    /// Closes what [`Drive::open_file`] opened.
    pub fn release(&self, id: &NodeId) {
        let id = &self.inner.canonical(id);
        let mut open = held(&self.inner.open);
        let mut closed = false;
        if let Some(n) = open.get_mut(id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(id);
                closed = true;
            }
        }
        drop(open);
        if closed {
            self.inner.closed(id);
        }
        // Off the caller's thread (a file system's): eviction reads the listing.
        let inner = Arc::clone(&self.inner);
        drop(self.inner.runtime.spawn_blocking(move || inner.evict()));
    }

    /// `len` bytes of `id` from `offset` (fewer at its end): what is here at once, the rest when
    /// its chunk has come.
    ///
    /// # Errors
    /// See [`ReadError`].
    pub async fn read(&self, id: &NodeId, offset: u64, len: u64) -> Result<Vec<u8>, ReadError> {
        let inner = &self.inner;
        let id = &inner.canonical(id);
        if let Some(edit) = inner.edit(id) {
            if edit.folder {
                return Err(ReadError::Folder);
            }
            if edit.dirty.is_some()
                && let Some(read) = inner.read_dirty(id, offset, len)
            {
                return read;
            }
        }
        let remote = match id {
            NodeId::Top => return Err(ReadError::Folder),
            NodeId::Local(_) => return Err(ReadError::Gone),
            NodeId::Remote(remote) => remote,
        };
        // The entry is read again each try: a newer version that replaced the transfer this
        // read waited on is joined, not fought, and one that went is gone.
        for _ in 0..8 {
            let entry = inner
                .entry_open(remote)
                .map_err(|_| ReadError::Cache)?
                .ok_or(ReadError::Gone)?;
            match &entry.kind {
                ItemKind::Folder => return Err(ReadError::Folder),
                ItemKind::Online { url, .. } => {
                    let link = online::link_file(url.as_str(), remote);
                    let from = usize::try_from(offset)
                        .unwrap_or(usize::MAX)
                        .min(link.len());
                    let to = from
                        .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                        .min(link.len());
                    return Ok(link[from..to].to_vec());
                }
                _ => {}
            }
            let end = offset.saturating_add(len).min(entry.size.0);
            if offset >= end {
                return Ok(Vec::new());
            }
            let covered = held(&inner.index).items.get(&remote.0).is_some_and(|row| {
                row.version == entry.version.0 && row.present.covers(offset, end)
            });
            if covered {
                return read_at(
                    &inner.content(remote, &entry.version.0),
                    offset,
                    end - offset,
                );
            }
            let Some(transfer) = inner.ensure(&entry, false)? else {
                continue;
            };
            let chunk = inner.chunk;
            {
                let mut state = held(&transfer.state);
                let first = offset / chunk * chunk;
                let mut starts: Vec<u64> = (first..end)
                    .step_by(usize::try_from(chunk).unwrap_or(usize::MAX))
                    .collect();
                starts.retain(|start| !state.present.covers(*start, (*start + chunk).min(end)));
                starts.reverse();
                state.wanted.extend(starts);
            }
            transfer.promoted.notify_one();
            let mut moved = transfer.moved.subscribe();
            loop {
                {
                    let state = held(&transfer.state);
                    if state.present.covers(offset, end) {
                        drop(state);
                        return read_at(&transfer.file, offset, end - offset);
                    }
                    if let Some(failure) = state.failure {
                        return Err(failure);
                    }
                    if state.cancelled {
                        break;
                    }
                }
                if moved.changed().await.is_err() {
                    break;
                }
            }
        }
        // It is there, but each try was overtaken by another change.
        Err(ReadError::Changed)
    }
}
