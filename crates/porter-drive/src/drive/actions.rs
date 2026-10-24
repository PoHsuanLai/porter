//! What a person asks of a drive's items (Download Now, Keep Downloaded, Remove Download), what
//! a new listing changes, and eviction.

use super::{Drive, Inner, NodeId, held};
use crate::listing::Entry;
use crate::words;
use porter_sync::{ItemKind, RemoteId};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Why an action did not take an item: words a person reads.
pub type Refusal = &'static str;

impl Inner {
    /// The files at or under `id` (a file: itself), from the listing.
    pub(super) fn files_under(&self, id: &RemoteId) -> Result<Vec<Entry>, Refusal> {
        let entry = self.entry(id).ok().flatten().ok_or(words::NOT_ON_A_DRIVE)?;
        Ok(match entry.kind {
            ItemKind::Folder => self
                .listing
                .descendants(id)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.kind.is_file())
                .collect(),
            ItemKind::File => vec![entry],
            _ => Vec::new(),
        })
    }

    /// Starts a download of each of `files` nobody waits on; whether one could not start.
    fn download_all(self: &Arc<Self>, files: &[Entry]) -> bool {
        let mut failed = false;
        for file in files {
            failed |= self.ensure(file, true).is_err();
        }
        failed
    }

    /// Drops what is cached of `id` and stops its download.
    fn drop_cached(&self, id: &RemoteId) -> bool {
        if let Some(transfer) = held(&self.transfers).remove(id) {
            transfer.cancel();
        }
        held(&self.errors).remove(id);
        let row = held(&self.index).items.remove(&id.0);
        if let Some(row) = &row {
            let _ = std::fs::remove_file(self.content(id, &row.version));
            self.touched();
        }
        row.is_some()
    }

    /// Removes what is past the limit: oldest opened first, never what is kept, open or being
    /// fetched.
    pub(super) fn evict(&self) {
        let limit = held(&self.limit).bytes_for(&self.dir);
        let gone: Vec<RemoteId> = {
            let index = held(&self.index);
            let mut total: u64 = index.items.values().map(|row| row.present.bytes()).sum();
            if total <= limit {
                return;
            }
            let transfers = held(&self.transfers);
            let mut rows: Vec<(&String, u64, u64)> = index
                .items
                .iter()
                .map(|(id, row)| (id, row.opened, row.present.bytes()))
                .collect();
            rows.sort_by_key(|(id, opened, _)| (*opened, (*id).clone()));
            let mut gone = Vec::new();
            for (id, _, bytes) in rows {
                if total <= limit {
                    break;
                }
                let remote = RemoteId(id.clone());
                let node = NodeId::Remote(remote.clone());
                if transfers.contains_key(&remote)
                    || self.is_open(&node)
                    || self.kept_in(&index, &remote)
                {
                    continue;
                }
                total = total.saturating_sub(bytes);
                gone.push(remote);
            }
            gone
        };
        for id in &gone {
            self.drop_cached(id);
        }
        self.emit(gone.into_iter().map(NodeId::Remote).collect());
    }
}

impl Drive {
    /// "Download Now": downloads `id` (a folder: every file under it). A file already here is
    /// taken and nothing happens.
    ///
    /// # Errors
    /// The words when it is refused: an online document, an item that is not there, or the
    /// drive cannot be reached now.
    pub fn download(&self, id: &NodeId) -> Result<(), Refusal> {
        let remote = match id {
            NodeId::Top => return Err(words::NOT_ON_A_DRIVE),
            NodeId::Local(n) => {
                return match self.inner.local_row(*n) {
                    Some(_) => Ok(()),
                    None => Err(words::NOT_ON_A_DRIVE),
                };
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = self
            .inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(words::NOT_ON_A_DRIVE)?;
        if matches!(entry.kind, ItemKind::Online { .. }) {
            return Err(words::ONLINE_ONLY);
        }
        let files = self.inner.files_under(remote)?;
        let missing: Vec<Entry> = files
            .into_iter()
            .filter(|file| !self.inner.here(file))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        if self.inner.offline.load(Ordering::SeqCst) {
            return Err(words::OFFLINE);
        }
        match self.inner.download_all(&missing) {
            true => Err(words::OFFLINE),
            false => Ok(()),
        }
    }

    /// "Keep Downloaded" on or off. On downloads `id` (a folder: everything under it, also what
    /// is added later) and never evicts it; off turns it off for everything under it too.
    ///
    /// # Errors
    /// The words when it is refused: an online document, an item that is not there, or turning
    /// off an item kept with its folder.
    pub fn keep(&self, id: &NodeId, keep: bool) -> Result<(), Refusal> {
        let inner = &self.inner;
        let remote = match id {
            NodeId::Top => return Err(words::NOT_ON_A_DRIVE),
            NodeId::Local(n) => {
                inner.local_row(*n).ok_or(words::NOT_ON_A_DRIVE)?;
                if !keep && inner.kept_by_folder(id) {
                    return Err(words::KEPT_WITH_FOLDER);
                }
                return Ok(());
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(words::NOT_ON_A_DRIVE)?;
        if matches!(entry.kind, ItemKind::Online { .. }) {
            return Err(words::ONLINE_ONLY);
        }
        if !keep && inner.kept_by_folder(id) {
            return Err(words::KEPT_WITH_FOLDER);
        }
        let under: Vec<Entry> = match entry.kind {
            ItemKind::Folder => inner.listing.descendants(remote).unwrap_or_default(),
            _ => Vec::new(),
        };
        {
            let mut index = held(&inner.index);
            match keep {
                true => {
                    index.kept.insert(remote.0.clone());
                }
                false => {
                    index.kept.remove(&remote.0);
                    for inside in &under {
                        index.kept.remove(&inside.id.0);
                    }
                }
            }
        }
        inner.touched();
        let mut changed: Vec<NodeId> = vec![id.clone()];
        changed.extend(under.iter().map(|e| NodeId::Remote(e.id.clone())));
        inner.emit(changed);
        if keep {
            let files = inner.files_under(remote)?;
            let missing: Vec<Entry> = files.into_iter().filter(|f| !inner.here(f)).collect();
            if inner.download_all(&missing) {
                return Err(words::OFFLINE);
            }
        } else {
            inner.evict();
        }
        Ok(())
    }

    /// "Remove Download": frees what `id` takes here (a folder: everything under it), which stays
    /// listed and downloads again when opened, and turns Keep Downloaded off for it.
    ///
    /// # Errors
    /// The words when it is refused: an online document, an item that is not there or is kept
    /// with its folder, or one with changes not sent yet (the first of a folder's refusals
    /// stands for it; the rest of it is still freed).
    pub fn remove_download(&self, id: &NodeId) -> Result<(), Refusal> {
        let inner = &self.inner;
        if self.inner.kept_by_folder(id) {
            return Err(words::KEPT_WITH_FOLDER);
        }
        let remote = match id {
            NodeId::Top => return Err(words::NOT_ON_A_DRIVE),
            NodeId::Local(n) => {
                inner.local_row(*n).ok_or(words::NOT_ON_A_DRIVE)?;
                return Err(words::WAITING);
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(words::NOT_ON_A_DRIVE)?;
        if matches!(entry.kind, ItemKind::Online { .. }) {
            return Err(words::ONLINE_ONLY);
        }
        let under: Vec<Entry> = match entry.kind {
            ItemKind::Folder => inner.listing.descendants(remote).unwrap_or_default(),
            _ => Vec::new(),
        };
        {
            let mut index = held(&inner.index);
            index.kept.remove(&remote.0);
            for inside in &under {
                index.kept.remove(&inside.id.0);
            }
        }
        inner.touched();
        let mut changed = vec![id.clone()];
        for file in std::iter::once(&entry).chain(under.iter()) {
            if file.kind.is_file() {
                inner.drop_cached(&file.id);
            }
            changed.push(NodeId::Remote(file.id.clone()));
        }
        inner.emit(changed);
        // What was made here under it stays: it is not on the drive yet.
        let folders: BTreeSet<String> = std::iter::once(&entry)
            .chain(under.iter())
            .filter(|e| e.kind == ItemKind::Folder)
            .map(|e| e.id.0.clone())
            .collect();
        let waiting = held(&inner.index)
            .local
            .iter()
            .any(|row| row.parent.as_ref().is_some_and(|p| folders.contains(p)));
        match waiting {
            true => Err(words::WAITING),
            false => Ok(()),
        }
    }

    /// A listing pass changed the drive: a cached copy of an item that is gone is dropped, one
    /// of an older version is stale (fetched again at once when kept, else on the next open),
    /// and everything under a kept folder that is not here, also what was just added, is
    /// fetched. Then eviction runs.
    pub fn listing_changed(&self) {
        let inner = &self.inner;
        let rows: Vec<(String, String)> = held(&inner.index)
            .items
            .iter()
            .map(|(id, row)| (id.clone(), row.version.clone()))
            .collect();
        let mut changed = Vec::new();
        for (id, version) in rows {
            let remote = RemoteId(id);
            match inner.entry(&remote) {
                Ok(None) => {
                    inner.drop_cached(&remote);
                    changed.push(NodeId::Remote(remote));
                }
                Ok(Some(entry)) if entry.version.0 != version => {
                    let open = inner
                        .transfer(&remote)
                        .is_some_and(|t| t.version == entry.version.0);
                    if !open {
                        inner.drop_cached(&remote);
                        changed.push(NodeId::Remote(remote));
                    }
                }
                _ => {}
            }
        }
        let kept: Vec<String> = held(&inner.index).kept.iter().cloned().collect();
        for id in kept {
            let remote = RemoteId(id);
            match inner.entry(&remote) {
                Ok(None) => {
                    held(&inner.index).kept.remove(&remote.0);
                    inner.touched();
                }
                Ok(Some(_)) => {
                    let missing: Vec<Entry> = inner
                        .files_under(&remote)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|file| !inner.here(file) && inner.transfer(&file.id).is_none())
                        .collect();
                    inner.download_all(&missing);
                }
                Err(_) => {}
            }
        }
        // A file made here whose folder is gone shows at the top: nothing made here is lost.
        {
            let mut index = held(&inner.index);
            let mut moved = Vec::new();
            for row in &mut index.local {
                if let Some(parent) = &row.parent
                    && matches!(inner.listing.entry(&RemoteId(parent.clone())), Ok(None))
                {
                    row.parent = None;
                    moved.push(NodeId::Local(row.n));
                }
            }
            if !moved.is_empty() {
                drop(index);
                inner.touched();
                changed.extend(moved);
            }
        }
        inner.emit(changed);
        inner.evict();
    }

    /// Bytes of the drive's files here: what is cached and what was made here.
    pub fn local_bytes(&self) -> u64 {
        let index = held(&self.inner.index);
        index
            .items
            .values()
            .map(|row| row.present.bytes())
            .chain(index.local.iter().map(|row| row.size))
            .sum()
    }
}
