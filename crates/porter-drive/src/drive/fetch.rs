//! Fetching: one transfer per item being fetched, filling its content file chunk by chunk from
//! the start, and a read that waits for the chunk it needs (asked for before the rest).

use super::{Drive, Inner, NodeId, held};
use crate::index::Row;
use crate::listing::Entry;
use crate::online;
use crate::ranges::Ranges;
use crate::source::SourceError;
use crate::words;
use porter_sync::{ItemKind, RemoteId};
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
    failure: Option<SourceError>,
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
                        },
                    );
                    Ranges::default()
                }
            }
        };
        let mut transfers = held(&self.transfers);
        if let Some(running) = transfers.get(&entry.id)
            && running.version == version
            && !held(&running.state).cancelled
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

    /// Ends `transfer` (done, failed or cancelled): it leaves the running ones.
    fn retire(&self, transfer: &Arc<Transfer>) {
        let mut transfers = held(&self.transfers);
        if transfers
            .get(&transfer.id)
            .is_some_and(|running| Arc::ptr_eq(running, transfer))
        {
            transfers.remove(&transfer.id);
        }
    }

    /// Notes `present` of `transfer` in the index, if it still names that version.
    fn note(&self, transfer: &Transfer, present: &Ranges) {
        let mut index = held(&self.index);
        if let Some(row) = index.items.get_mut(&transfer.id.0)
            && row.version == transfer.version
        {
            row.present = present.clone();
        }
        drop(index);
        self.touched();
    }
}

/// Fills `transfer`'s file: the chunks readers wait for first, then the rest from the start.
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
            finish(&inner, &transfer);
            return;
        };
        let len = chunk.min(transfer.size - start);
        let source = Arc::clone(&inner.source);
        drop(inner);
        let fetched = source.fetch(&transfer.id, start, len).await;
        let Some(inner) = weak.upgrade() else { return };
        let written = match fetched {
            Ok(bytes) if bytes.is_empty() => Err(SourceError::Refused),
            Ok(bytes) => {
                let bytes = &bytes[..bytes.len().min(usize::try_from(len).unwrap_or(usize::MAX))];
                let wrote = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&transfer.file)
                    .and_then(|file| file.write_all_at(bytes, start));
                match wrote {
                    Ok(()) => Ok(bytes.len() as u64),
                    // The file went (the download was removed): nothing more to do.
                    Err(_) => {
                        transfer.cancel();
                        inner.retire(&transfer);
                        return;
                    }
                }
            }
            Err(error) => Err(error),
        };
        match written {
            Ok(n) => {
                inner.offline.store(false, Ordering::SeqCst);
                let present = {
                    let mut state = held(&transfer.state);
                    if state.cancelled {
                        continue;
                    }
                    state.present.insert(start, start + n);
                    state.cursor = start + n;
                    state.present.clone()
                };
                inner.note(&transfer, &present);
                transfer.moved.send_modify(|n| *n += 1);
                inner.emit(vec![NodeId::Remote(transfer.id.clone())]);
            }
            Err(error) => {
                if error == SourceError::Offline {
                    inner.offline.store(true, Ordering::SeqCst);
                }
                held(&transfer.state).failure = Some(error);
                inner.retire(&transfer);
                held(&inner.errors).insert(transfer.id.clone(), words::OFFLINE.to_owned());
                transfer.moved.send_modify(|n| *n += 1);
                inner.emit(vec![NodeId::Remote(transfer.id.clone())]);
                return;
            }
        }
    }
}

fn finish(inner: &Arc<Inner>, transfer: &Arc<Transfer>) {
    inner.note(transfer, &Ranges::whole(transfer.size));
    inner.retire(transfer);
    transfer.moved.send_modify(|n| *n += 1);
    inner.emit(vec![NodeId::Remote(transfer.id.clone())]);
    inner.evict();
}

impl Drive {
    /// Opens `id` for reading: a file not here starts downloading from its start, and it is not
    /// evicted until [`Drive::release`]. Its last opened time is now.
    ///
    /// # Errors
    /// It is a folder, it is not there, or its download could not start.
    pub fn open_file(&self, id: &NodeId) -> Result<(), ReadError> {
        let inner = &self.inner;
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
        let mut open = held(&self.inner.open);
        if let Some(n) = open.get_mut(id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                open.remove(id);
            }
        }
        drop(open);
        self.inner.evict();
    }

    /// `len` bytes of `id` from `offset` (fewer at its end): what is here at once, the rest when
    /// its chunk has come.
    ///
    /// # Errors
    /// See [`ReadError`].
    pub async fn read(&self, id: &NodeId, offset: u64, len: u64) -> Result<Vec<u8>, ReadError> {
        let inner = &self.inner;
        let remote = match id {
            NodeId::Top => return Err(ReadError::Folder),
            NodeId::Local(n) => {
                let row = inner.local_row(*n).ok_or(ReadError::Gone)?;
                let end = offset.saturating_add(len).min(row.size);
                return match offset < end {
                    true => read_at(&inner.local_file(*n), offset, end - offset),
                    false => Ok(Vec::new()),
                };
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = inner
            .entry(remote)
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
        let here = inner.content(remote, &entry.version.0);
        for _ in 0..8 {
            let covered = held(&inner.index).items.get(&remote.0).is_some_and(|row| {
                row.version == entry.version.0 && row.present.covers(offset, end)
            });
            if covered {
                return read_at(&here, offset, end - offset);
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
                        return Err(failure.into());
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
        Err(ReadError::Gone)
    }
}
