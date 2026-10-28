//! What a person asks of a drive's items (Download Now, Keep Downloaded, Remove Download) and of
//! the whole drive (Keep Everything Downloaded), what a new listing changes, and eviction.

use super::{Drive, FreeSpace, Inner, NodeId, held};
use crate::Reason;
use crate::listing::Entry;
use porter_sync::{ItemKind, RemoteId};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl Inner {
    /// The files at or under `id` (a file: itself), from the listing.
    pub(super) fn files_under(&self, id: &RemoteId) -> Result<Vec<Entry>, Reason> {
        let entry = self.entry(id).ok().flatten().ok_or(Reason::NotOnADrive)?;
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

    /// Every entry of the drive, at any depth, from the listing.
    fn every_entry(&self) -> Vec<Entry> {
        let mut all = Vec::new();
        for entry in self.listing.children(None).unwrap_or_default() {
            if entry.kind == ItemKind::Folder {
                all.extend(self.listing.descendants(&entry.id).unwrap_or_default());
            }
            all.push(entry);
        }
        all
    }

    /// How many bytes the cache's file system has free (what a person may use, not what is kept
    /// for the system), as [`super::DriveConfig::free_space`] says to find it; `None` when it is
    /// not known (a file system that cannot be asked).
    fn free_bytes(&self) -> Option<u64> {
        match self.free_space {
            FreeSpace::Bytes(bytes) => Some(bytes),
            FreeSpace::Unknown => None,
            FreeSpace::Measured => self
                .dir
                .ancestors()
                .find_map(|at| rustix::fs::statvfs(at).ok())
                .map(|fs| fs.f_bavail.saturating_mul(fs.f_frsize)),
        }
    }

    /// Notes whether keeping everything is short of room, telling the watchers only of a change.
    fn set_short_of_room(&self, short: bool) {
        self.short_of_room
            .send_if_modified(|was| std::mem::replace(was, short) != short);
    }

    /// While the whole drive is kept: starts a download of each file not here and not on its
    /// way, when what is still to come fits in the free space; when it does not, nothing is
    /// started (and nothing is removed) and the drive says it is short of room until a later
    /// look finds the room. A free space that is not known is never short.
    pub(super) fn fetch_everything(self: &Arc<Self>) {
        if !self.keeps_all() {
            self.set_short_of_room(false);
            return;
        }
        let files: Vec<Entry> = self
            .every_entry()
            .into_iter()
            .filter(|e| e.kind.is_file() && !self.here(e))
            .collect();
        let coming: u64 = {
            let index = held(&self.index);
            files
                .iter()
                .map(|file| {
                    let present = index
                        .items
                        .get(&file.id.0)
                        .filter(|row| row.version == file.version.0)
                        .map_or(0, |row| row.present.bytes());
                    file.size.0.saturating_sub(present)
                })
                .sum()
        };
        let short = self.free_bytes().is_some_and(|free| coming > free);
        self.set_short_of_room(short);
        if short {
            return;
        }
        let missing: Vec<Entry> = files
            .into_iter()
            .filter(|file| self.transfer(&file.id).is_none())
            .collect();
        self.download_all(&missing);
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

    /// Removes what is past the limit, which counts what is not kept downloaded: oldest opened
    /// first, never what is kept, open or being fetched, and never what was changed here and is
    /// not settled on the drive yet (bytes written here are in `local/`, outside the limit).
    pub(super) fn evict(&self) {
        let limit = held(&self.limit).bytes_for(&self.dir);
        let gone: Vec<RemoteId> = {
            let index = held(&self.index);
            let mut rows: Vec<(&String, u64, u64)> = index
                .items
                .iter()
                .filter(|(id, _)| {
                    !index
                        .edits
                        .contains_key(&NodeId::Remote(RemoteId((*id).clone())))
                })
                .filter(|(id, _)| !self.kept_in(&index, &RemoteId((*id).clone())))
                .map(|(id, row)| (id, row.opened, row.present.bytes()))
                .collect();
            let mut total: u64 = rows.iter().map(|(_, _, bytes)| bytes).sum();
            if total <= limit {
                return;
            }
            let transfers = held(&self.transfers);
            rows.sort_by_key(|(id, opened, _)| (*opened, (*id).clone()));
            let mut gone = Vec::new();
            for (id, _, bytes) in rows {
                if total <= limit {
                    break;
                }
                let remote = RemoteId(id.clone());
                let node = NodeId::Remote(remote.clone());
                if transfers.contains_key(&remote) || self.is_open(&node) {
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
    /// The reason when it is refused: an online document, an item that is not there, or the
    /// drive cannot be reached now.
    pub fn download(&self, id: &NodeId) -> Result<(), Reason> {
        let id = &self.inner.canonical(id);
        let remote = match id {
            NodeId::Top => return Err(Reason::NotOnADrive),
            NodeId::Local(_) => {
                return match self.inner.edit(id) {
                    Some(_) => Ok(()),
                    None => Err(Reason::NotOnADrive),
                };
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = self
            .inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(Reason::NotOnADrive)?;
        if matches!(entry.kind, ItemKind::Online { .. }) {
            return Err(Reason::OnlineOnly);
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
            return Err(Reason::Offline);
        }
        match self.inner.download_all(&missing) {
            true => Err(Reason::Offline),
            false => Ok(()),
        }
    }

    /// "Keep Downloaded" on or off. On downloads `id` (a folder: everything under it, also what
    /// is added later) and never evicts it; off turns it off for everything under it too.
    ///
    /// # Errors
    /// The reason when it is refused: an online document, an item that is not there, or turning
    /// off an item kept with its folder.
    pub fn keep(&self, id: &NodeId, keep: bool) -> Result<(), Reason> {
        let inner = &self.inner;
        let id = &inner.canonical(id);
        let remote = match id {
            NodeId::Top => return Err(Reason::NotOnADrive),
            NodeId::Local(_) => {
                inner.edit(id).ok_or(Reason::NotOnADrive)?;
                if !keep && inner.keeps_all() {
                    return Err(Reason::KeptWithDrive);
                }
                if !keep && inner.kept_by_folder(id) {
                    return Err(Reason::KeptWithFolder);
                }
                return Ok(());
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(Reason::NotOnADrive)?;
        if matches!(entry.kind, ItemKind::Online { .. }) {
            return Err(Reason::OnlineOnly);
        }
        if !keep && inner.keeps_all() {
            return Err(Reason::KeptWithDrive);
        }
        if !keep && inner.kept_by_folder(id) {
            return Err(Reason::KeptWithFolder);
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
                return Err(Reason::Offline);
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
    /// The reason when it is refused: an online document, an item that is not there or is kept
    /// with its folder, or one with changes not sent yet (the first of a folder's refusals
    /// stands for it; the rest of it is still freed).
    pub fn remove_download(&self, id: &NodeId) -> Result<(), Reason> {
        let inner = &self.inner;
        let id = &inner.canonical(id);
        if inner.keeps_all() && *id != NodeId::Top {
            return Err(Reason::KeptWithDrive);
        }
        if self.inner.kept_by_folder(id) {
            return Err(Reason::KeptWithFolder);
        }
        let remote = match id {
            NodeId::Top => return Err(Reason::NotOnADrive),
            NodeId::Local(_) => {
                inner.edit(id).ok_or(Reason::NotOnADrive)?;
                return match inner.other(id) {
                    Some(_) => Err(Reason::Conflict),
                    None => Err(Reason::Waiting),
                };
            }
            NodeId::Remote(remote) => remote,
        };
        let entry = inner
            .entry(remote)
            .ok()
            .flatten()
            .ok_or(Reason::NotOnADrive)?;
        if matches!(entry.kind, ItemKind::Online { .. }) {
            return Err(Reason::OnlineOnly);
        }
        if inner.other(id).is_some() {
            return Err(Reason::Conflict);
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
        let mut waiting = false;
        for file in std::iter::once(&entry).chain(under.iter()) {
            // What was changed here stays: it is not on the drive yet.
            if inner.edit(&NodeId::Remote(file.id.clone())).is_some() {
                waiting |= file.kind.is_file();
                continue;
            }
            if file.kind.is_file() {
                inner.drop_cached(&file.id);
            }
            changed.push(NodeId::Remote(file.id.clone()));
        }
        inner.emit(changed);
        let folders: BTreeSet<NodeId> = std::iter::once(&entry)
            .chain(under.iter())
            .filter(|e| e.kind == ItemKind::Folder)
            .map(|e| NodeId::Remote(e.id.clone()))
            .collect();
        waiting |= held(&inner.index)
            .edits
            .values()
            .any(|edit| !edit.removed && folders.contains(&edit.parent));
        match waiting {
            true => Err(Reason::Waiting),
            false => Ok(()),
        }
    }

    /// A listing pass changed the drive: a cached copy of an item that is gone is dropped, one
    /// of an older version is stale (fetched again at once when kept, else on the next open),
    /// and everything under a kept folder that is not here, also what was just added, is
    /// fetched. Then eviction runs.
    ///
    /// Call it after a pass that ended caught up: what an unfinished listing lacks is not gone.
    /// A listing with nothing at its top, or that cannot be read, is taken as not listed yet,
    /// and nothing is dropped against it.
    pub fn listing_changed(&self) {
        let inner = &self.inner;
        if inner
            .listing
            .children(None)
            .map_or(true, |top| top.is_empty())
        {
            return;
        }
        held(&inner.stale).clear();
        let met = inner.meet_the_drive();
        inner.settle();
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
        // What was made or moved here into a folder that is gone shows at the top: nothing made
        // here is lost.
        let placed: Vec<(NodeId, RemoteId)> = held(&inner.index)
            .edits
            .values()
            .filter(|edit| !edit.removed)
            .filter_map(|edit| match &edit.parent {
                NodeId::Remote(parent) => Some((edit.id.clone(), parent.clone())),
                _ => None,
            })
            .collect();
        let mut moved = Vec::new();
        for (id, parent) in placed {
            let gone = !held(&inner.index)
                .edits
                .contains_key(&NodeId::Remote(parent.clone()))
                && matches!(inner.listing.entry(&parent), Ok(None));
            if gone && let Some(edit) = held(&inner.index).edits.get_mut(&id) {
                edit.parent = NodeId::Top;
                moved.push(id);
            }
        }
        if !moved.is_empty() {
            inner.touched();
            inner.wake.notify_one();
            changed.extend(moved);
        }
        changed.extend(met);
        inner.emit(changed);
        // While the whole drive is kept, what the listing just added is fetched too.
        inner.fetch_everything();
        inner.evict();
    }

    /// "Keep Everything Downloaded" on or off. On: every item of the drive is kept, downloaded
    /// and never evicted, also what the listing adds later, as long as what is still to come fits
    /// in the free space ([`Drive::short_of_room`] says when it does not; nothing is removed for
    /// it). Off: back to the items kept one by one. Nothing is removed then; the rest may be
    /// evicted again past the limit.
    ///
    /// The drive does not remember it: its host sets it again each time it opens the drive.
    pub fn set_keep_all(&self, on: bool) {
        let inner = &self.inner;
        let was = inner.keep_all.swap(on, Ordering::SeqCst);
        if was != on {
            let every: Vec<NodeId> = inner
                .every_entry()
                .into_iter()
                .map(|entry| NodeId::Remote(entry.id))
                .collect();
            inner.emit(every);
        }
        inner.fetch_everything();
        if !on {
            inner.evict();
        }
    }

    /// Whether the whole drive is kept downloaded ([`Drive::set_keep_all`]).
    pub fn keeps_all(&self) -> bool {
        self.inner.keeps_all()
    }

    /// Whether keeping the whole drive downloaded needs more room than the cache's file system
    /// has free, as the last look found ([`crate::Reason::NoRoomForAll`]): what is not here is
    /// then not fetched until it fits. Always `false` while the drive is not kept whole.
    pub fn short_of_room(&self) -> bool {
        *self.inner.short_of_room.borrow()
    }

    /// Hears [`Drive::short_of_room`] change, each time it does and only then.
    pub fn watch_room(&self) -> tokio::sync::watch::Receiver<bool> {
        self.inner.short_of_room.subscribe()
    }

    /// Bytes of the drive's files here: what is cached and what was written here.
    pub fn local_bytes(&self) -> u64 {
        let index = held(&self.inner.index);
        index
            .items
            .values()
            .map(|row| row.present.bytes())
            .chain(
                index
                    .edits
                    .values()
                    .filter_map(|edit| edit.dirty.as_ref().map(|dirty| dirty.size)),
            )
            .sum()
    }
}
