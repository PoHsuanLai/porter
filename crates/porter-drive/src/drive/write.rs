//! Changes made here: new files and folders, bytes written, sizes, renames and moves (a save by
//! rename over a file among them), and removals. Each is kept in the cache at once and in the
//! index's queue of changes, and sent after (see `send`).
//!
//! - **Bytes.** A file changed here is written to `local/<n>`: a copy of the drive's version
//!   made when it is first opened for writing (downloaded first when it is not all here), or a
//!   new file. A sync makes those bytes durable (and the index that names them); it does not
//!   send them.
//! - **Saving by rename.** A file renamed over a file of the drive's gives that file its bytes:
//!   the same item, a new version, so the drive keeps its history. It is never a removal and a
//!   new file.
//! - **Removals** go to the drive's recycle bin. An item made here and removed before it was sent
//!   is never sent.
//! - **Names** are given back to the drive by [`naming::drive_name`]'s rule, and one the drive
//!   refuses is refused here, when it is made ([`WriteError::BadName`]), except on what stays on
//!   this computer.
//! - **The desktop's trash** (see [`crate::local_only`]): an item of the drive's moved into it
//!   goes to the drive's recycle bin and its bytes stay in the trash, as a copy made here
//!   (downloaded first when they are not all here); moved back out, the copy is sent as a new
//!   file; emptying the trash removes only those bytes.

use super::{
    Drive, DriveError, Inner, NodeId, NodeKind, ReadError, Writing, held, key_text, now_secs,
};
use crate::index::{Base, Dirty, Edit};
use crate::naming;
use porter_sync::{ItemKind, RemoteId};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Instant;

/// Why a change was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// The drive is shown read-only here.
    #[error("the drive is shown read-only")]
    ReadOnly,
    /// It cannot be changed here: an online document's content, or a kind of file a drive does
    /// not hold.
    #[error("it cannot be changed here")]
    NotAllowed,
    /// The drive does not take this name.
    #[error("the drive does not take this name")]
    BadName,
    /// Something there has this name.
    #[error("the name is taken")]
    Exists,
    /// It is not there (any more).
    #[error("it is not there")]
    Gone,
    /// The folder is not empty.
    #[error("the folder is not empty")]
    NotEmpty,
    /// It is a folder.
    #[error("it is a folder")]
    Folder,
    /// It is not a folder.
    #[error("it is not a folder")]
    NotFolder,
    /// A folder cannot go inside itself.
    #[error("a folder cannot go inside itself")]
    Inside,
    /// Its bytes were needed and could not be read.
    #[error("its bytes could not be read: {0}")]
    Read(ReadError),
    /// The cache could not be written.
    #[error("the cache could not be written")]
    Cache,
}

impl From<DriveError> for WriteError {
    fn from(_: DriveError) -> Self {
        WriteError::Cache
    }
}

impl From<crate::listing::ListingError> for WriteError {
    fn from(_: crate::listing::ListingError) -> Self {
        WriteError::Cache
    }
}

impl From<std::io::Error> for WriteError {
    fn from(_: std::io::Error) -> Self {
        WriteError::Cache
    }
}

/// A new, empty, owner-only file at `path`.
fn new_file(path: &PathBuf) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map(drop)
}

impl Inner {
    fn writable(&self) -> Result<(), WriteError> {
        match self.sink {
            Some(_) => Ok(()),
            None => Err(WriteError::ReadOnly),
        }
    }

    /// Refuses `name` for an item in `folder` when the drive would refuse it and it is to be
    /// sent.
    fn check_name(&self, folder: &NodeId, name: &str, file: bool) -> Result<(), WriteError> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(WriteError::BadName);
        }
        let stays = (file && crate::local_only::is_lock_file(name))
            || (*folder == NodeId::Top && crate::local_only::is_trash(name))
            || (*folder != NodeId::Top && self.local_only(folder));
        match (stays, &self.sink) {
            (false, Some(sink)) if !sink.accepts_name(name) => Err(WriteError::BadName),
            _ => Ok(()),
        }
    }

    /// `folder` as a folder to make something in.
    fn folder_for(&self, folder: &NodeId) -> Result<NodeId, WriteError> {
        let folder = self.canonical(folder);
        match self.attrs(&folder)? {
            Some(attrs) if attrs.kind == NodeKind::Folder => Ok(folder),
            Some(_) => Err(WriteError::NotFolder),
            None => Err(WriteError::Gone),
        }
    }

    /// Notes a change in the queue: its place is after everything queued before.
    fn queue(&self, edit: &mut Edit) {
        let mut index = held(&self.index);
        edit.seq = index.take_seq();
    }

    /// Puts `edit` in the index, and wakes the sender.
    fn put_edit(&self, edit: Edit) {
        held(&self.index).edits.insert(edit.id.clone(), edit);
        self.touched();
        self.wake.notify_one();
    }

    /// The change of the drive's item `id` made here, or a new one against the listing's entry.
    fn edit_of_remote(&self, id: &RemoteId) -> Result<Edit, WriteError> {
        let node = NodeId::Remote(id.clone());
        if let Some(edit) = self.edit(&node) {
            return match edit.removed {
                true => Err(WriteError::Gone),
                false => Ok(edit),
            };
        }
        let entry = self.listing.entry(id)?.ok_or(WriteError::Gone)?;
        let mut edit = Edit {
            id: node,
            folder: entry.kind == ItemKind::Folder,
            parent: entry.parent.clone().map_or(NodeId::Top, NodeId::Remote),
            name: entry.name.clone(),
            base: Some(Base {
                version: entry.version.0.clone(),
                parent: entry.parent.map(|p| p.0),
                name: entry.name,
                size: entry.size.0,
                modified: entry.modified.map(|m| m.0),
            }),
            dirty: None,
            removed: false,
            session: None,
            awaiting: None,
            made: now_secs(),
            seq: 0,
            refused: None,
        };
        self.queue(&mut edit);
        Ok(edit)
    }

    /// Makes `id`'s bytes here to change: `keep` of the drive's version's first bytes copied (all
    /// of them with `None`), downloaded first when they are not here.
    async fn make_dirty(
        &self,
        drive: &Drive,
        id: &NodeId,
        keep: Option<u64>,
    ) -> Result<(), WriteError> {
        let id = self.canonical(id);
        if self.edit(&id).is_some_and(|edit| edit.dirty.is_some()) {
            return Ok(());
        }
        let NodeId::Remote(remote) = &id else {
            return Err(WriteError::Gone);
        };
        let entry = self.entry(remote)?.ok_or(WriteError::Gone)?;
        match entry.kind {
            ItemKind::Folder => return Err(WriteError::Folder),
            ItemKind::Online { .. } => return Err(WriteError::NotAllowed),
            _ => {}
        }
        let size = keep.unwrap_or(entry.size.0).min(entry.size.0);
        let n = held(&self.index).take_local();
        let path = self.local_file(n);
        new_file(&path)?;
        let cached = held(&self.index)
            .items
            .get(&remote.0)
            .is_some_and(|row| row.version == entry.version.0 && row.present.covers(0, size));
        let copied = match cached {
            true => copy_span(&self.content(remote, &entry.version.0), &path, size),
            false => self.download_into(drive, &id, &path, size).await,
        };
        if let Err(why) = copied {
            let _ = std::fs::remove_file(&path);
            return Err(why);
        }
        let mut edit = self.edit_of_remote(remote)?;
        if edit.dirty.is_some() {
            // Made dirty by another call meanwhile: its copy stands.
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        edit.dirty = Some(Dirty {
            file: n,
            size,
            modified: entry.modified.map_or_else(now_secs, |m| m.0),
            writes: u64::from(keep.is_some_and(|k| k != entry.size.0)),
        });
        self.wrote(&path);
        self.put_edit(edit);
        Ok(())
    }

    /// Writes the first `size` bytes of `id` into `path`, read through the cache.
    async fn download_into(
        &self,
        drive: &Drive,
        id: &NodeId,
        path: &PathBuf,
        size: u64,
    ) -> Result<(), WriteError> {
        let file = std::fs::OpenOptions::new().write(true).open(path)?;
        let mut at = 0;
        while at < size {
            let len = self.chunk.min(size - at);
            let bytes = drive.read(id, at, len).await.map_err(WriteError::Read)?;
            if bytes.is_empty() {
                return Err(WriteError::Read(ReadError::Changed));
            }
            file.write_all_at(&bytes, at)?;
            at += bytes.len() as u64;
        }
        Ok(())
    }

    /// Notes that `id` was written or closed now: its quiet time starts again.
    fn touched_write(&self, id: &NodeId) {
        if let Some(writing) = held(&self.writing).get_mut(id) {
            writing.last = Instant::now();
        }
    }

    /// Moves what is known of `from` in memory to `to`, which it became.
    pub(super) fn rekey(&self, from: &NodeId, to: &NodeId) {
        let moved = held(&self.writing).remove(from);
        if let Some(writing) = moved {
            let mut all = held(&self.writing);
            let at = all.entry(to.clone()).or_insert(Writing {
                handles: 0,
                last: writing.last,
            });
            at.handles += writing.handles;
            at.last = at.last.max(writing.last);
        }
        let opened = held(&self.open).remove(from);
        if let Some(n) = opened {
            *held(&self.open).entry(to.clone()).or_default() += n;
        }
    }

    /// `from`, made here, is now `to`: it keeps its inode number, what was in it is in `to`, and
    /// its number names `to` from now on.
    pub(super) fn alias(&self, from: u64, to: &NodeId) {
        let local = NodeId::Local(from);
        let ino = self.ino(&local);
        {
            let mut index = held(&self.index);
            index.aliases.insert(from, to.clone());
            index.inos.insert(key_text(to), ino);
            for edit in index.edits.values_mut() {
                if edit.parent == local {
                    edit.parent = to.clone();
                }
            }
            let pairs: Vec<(NodeId, NodeId)> = index
                .conflicts
                .iter()
                .filter(|(a, b)| **a == local || **b == local)
                .map(|(a, b)| (a.clone(), b.clone()))
                .collect();
            for (a, b) in pairs {
                index.conflicts.remove(&a);
                let swap = |x: NodeId| if x == local { to.clone() } else { x };
                index.conflicts.insert(swap(a), swap(b));
            }
        }
        held(&self.inodes).insert(ino, to.clone());
        self.rekey(&local, to);
        self.touched();
    }

    /// Drops what was made here as `n` (never sent): its bytes and its change.
    fn forget_made(&self, n: u64) {
        let gone = held(&self.index).edits.remove(&NodeId::Local(n));
        if let Some(dirty) = gone.and_then(|edit| edit.dirty) {
            let _ = std::fs::remove_file(self.local_file(dirty.file));
        }
        held(&self.writing).remove(&NodeId::Local(n));
        self.touched();
    }

    /// Whether `id` is `inside` or in it, at any depth.
    fn within(&self, id: &NodeId, inside: &NodeId) -> bool {
        let mut at = self.canonical(id);
        for _ in 0..super::DEPTH {
            if at == *inside {
                return true;
            }
            match self.parent_of(&at) {
                Ok(Some(NodeId::Top)) | Ok(None) | Err(_) => return false,
                Ok(Some(parent)) => at = parent,
            }
        }
        false
    }
}

/// Copies the first `size` bytes of `from` to `to`.
fn copy_span(from: &PathBuf, to: &PathBuf, size: u64) -> Result<(), WriteError> {
    let source = std::fs::File::open(from)?;
    let target = std::fs::OpenOptions::new().write(true).open(to)?;
    let mut at = 0;
    let mut buffer = vec![0; 1 << 20];
    while at < size {
        let len = usize::try_from((size - at).min(buffer.len() as u64)).unwrap_or(0);
        source.read_exact_at(&mut buffer[..len], at)?;
        target.write_all_at(&buffer[..len], at)?;
        at += len as u64;
    }
    target.set_len(size)?;
    Ok(())
}

impl Drive {
    /// Makes an empty file `name` in `folder`, open for writing (as [`Drive::open_write`]
    /// opens; close it with [`Drive::release_write`]).
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn create(&self, folder: &NodeId, name: &str) -> Result<NodeId, WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let folder = inner.folder_for(folder)?;
        inner.check_name(&folder, name, true)?;
        if self.lookup(&folder, name)?.is_some() {
            return Err(WriteError::Exists);
        }
        let n = held(&inner.index).take_local();
        new_file(&inner.local_file(n))?;
        let now = now_secs();
        let dirty = Dirty {
            file: n,
            size: 0,
            modified: now,
            writes: 1,
        };
        let mut edit = Edit::made_here(n, folder.clone(), name.to_owned(), Some(dirty), now);
        inner.queue(&mut edit);
        let id = edit.id.clone();
        held(&inner.writing).insert(
            id.clone(),
            Writing {
                handles: 1,
                last: Instant::now(),
            },
        );
        *held(&inner.open).entry(id.clone()).or_default() += 1;
        inner.wrote(&inner.local_file(n));
        inner.put_edit(edit);
        inner.ino(&id);
        inner.emit(vec![id.clone(), folder]);
        Ok(id)
    }

    /// Makes a folder `name` in `folder`.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn make_folder(&self, folder: &NodeId, name: &str) -> Result<NodeId, WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let folder = inner.folder_for(folder)?;
        inner.check_name(&folder, name, false)?;
        if self.lookup(&folder, name)?.is_some() {
            return Err(WriteError::Exists);
        }
        let n = held(&inner.index).take_local();
        let mut edit = Edit::made_here(n, folder.clone(), name.to_owned(), None, now_secs());
        inner.queue(&mut edit);
        let id = edit.id.clone();
        inner.put_edit(edit);
        inner.ino(&id);
        inner.emit(vec![id.clone(), folder]);
        Ok(id)
    }

    /// Opens the file `id` for writing (`truncate`: emptied), as many times as it is asked; each
    /// open is closed with [`Drive::release_write`]. The drive's version is copied to change
    /// (downloaded first when it is not all here); it is not evicted while open.
    ///
    /// # Errors
    /// See [`WriteError`]: an online document's content is never written.
    pub async fn open_write(&self, id: &NodeId, truncate: bool) -> Result<(), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        match inner.attrs(&id)?.map(|attrs| attrs.kind) {
            None => return Err(WriteError::Gone),
            Some(NodeKind::Folder) => return Err(WriteError::Folder),
            Some(NodeKind::Online { .. }) => return Err(WriteError::NotAllowed),
            Some(NodeKind::File) => {}
        }
        *held(&inner.open).entry(id.clone()).or_default() += 1;
        let made = match truncate {
            true => self.set_len(&id, 0).await,
            false => inner.make_dirty(self, &id, None).await,
        };
        if let Err(why) = made {
            self.release(&id);
            return Err(why);
        }
        let mut writing = held(&inner.writing);
        let at = writing.entry(id).or_insert(Writing {
            handles: 0,
            last: Instant::now(),
        });
        at.handles += 1;
        at.last = Instant::now();
        Ok(())
    }

    /// Writes `bytes` into the file `id` at `offset`; how many were written.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn write(&self, id: &NodeId, offset: u64, bytes: &[u8]) -> Result<u64, WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        let mut edit = inner.edit(&id).ok_or(WriteError::Gone)?;
        if edit.removed {
            return Err(WriteError::Gone);
        }
        if edit.dirty.is_none() {
            // Sent while it was open: the bytes it sent are the cache's now.
            let NodeId::Remote(remote) = &id else {
                return Err(WriteError::Gone);
            };
            let base = edit.base.clone().ok_or(WriteError::Gone)?;
            let content = inner.content(remote, &base.version);
            let n = held(&inner.index).take_local();
            let path = inner.local_file(n);
            new_file(&path)?;
            copy_span(&content, &path, base.size)?;
            edit.dirty = Some(Dirty {
                file: n,
                size: base.size,
                modified: now_secs(),
                writes: 0,
            });
        }
        let Some(dirty) = edit.dirty.as_mut() else {
            return Err(WriteError::Gone);
        };
        let path = inner.local_file(dirty.file);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|file| file.write_all_at(bytes, offset))?;
        dirty.size = dirty.size.max(offset + bytes.len() as u64);
        dirty.modified = now_secs();
        dirty.writes += 1;
        edit.refused = None;
        inner.wrote(&path);
        inner.touched_write(&id);
        inner.put_edit(edit);
        inner.emit(vec![id]);
        Ok(bytes.len() as u64)
    }

    /// Sets the size of the file `id`: cut, or grown with zeros.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub async fn set_len(&self, id: &NodeId, size: u64) -> Result<(), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        inner.make_dirty(self, &id, Some(size)).await?;
        let mut edit = inner.edit(&id).ok_or(WriteError::Gone)?;
        let Some(dirty) = edit.dirty.as_mut() else {
            return Err(WriteError::Gone);
        };
        let path = inner.local_file(dirty.file);
        if dirty.size != size {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_len(size))?;
            dirty.size = size;
            dirty.writes += 1;
            dirty.modified = now_secs();
        }
        edit.refused = None;
        inner.wrote(&path);
        inner.touched_write(&id);
        inner.put_edit(edit);
        inner.emit(vec![id]);
        Ok(())
    }

    /// Sets when the file `id` last changed, as a file system's `utimes` does: kept with bytes
    /// changed here, and otherwise taken and not sent (a drive keeps its own).
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn set_modified(&self, id: &NodeId, at: i64) -> Result<(), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        if let Some(mut edit) = inner.edit(&id)
            && let Some(dirty) = edit.dirty.as_mut()
        {
            dirty.modified = at;
            held(&inner.index).edits.insert(id.clone(), edit);
            inner.touched();
            inner.emit(vec![id]);
            return Ok(());
        }
        match inner.attrs(&id)? {
            Some(_) => Ok(()),
            None => Err(WriteError::Gone),
        }
    }

    /// Makes what was written to `id` durable here, with the index that names it. It is not
    /// sent any sooner.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn sync(&self, id: &NodeId) -> Result<(), WriteError> {
        let inner = &self.inner;
        let id = inner.canonical(id);
        if let Some(dirty) = inner.edit(&id).and_then(|edit| edit.dirty) {
            std::fs::File::open(inner.local_file(dirty.file)).and_then(|f| f.sync_data())?;
        }
        inner.flush_now()?;
        Ok(())
    }

    /// Closes what [`Drive::open_write`] or [`Drive::create`] opened. Once nothing has it open,
    /// it is sent after the quiet time; a file opened and closed with nothing written is left as
    /// the drive has it.
    pub fn release_write(&self, id: &NodeId) {
        let inner = &self.inner;
        let id = inner.canonical(id);
        let closed = {
            let mut writing = held(&inner.writing);
            match writing.get_mut(&id) {
                Some(at) => {
                    at.handles = at.handles.saturating_sub(1);
                    at.last = Instant::now();
                    at.handles == 0
                }
                None => true,
            }
        };
        if closed && let Some(mut edit) = inner.edit(&id) {
            let untouched =
                edit.base.is_some() && edit.dirty.as_ref().is_some_and(|d| d.writes == 0);
            if untouched {
                if let Some(dirty) = edit.dirty.take() {
                    let _ = std::fs::remove_file(inner.local_file(dirty.file));
                }
                let mut index = held(&inner.index);
                match edit.pending() || edit.awaiting.is_some() {
                    true => index.edits.insert(id.clone(), edit),
                    false => index.edits.remove(&id),
                };
                drop(index);
                inner.touched();
                inner.emit(vec![id.clone()]);
            }
        }
        inner.wake.notify_one();
        self.release(&id);
    }

    /// Removes the file `id`: an item of the drive's goes to its recycle bin, one made here and
    /// not sent yet is gone.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn remove(&self, id: &NodeId) -> Result<(), WriteError> {
        let id = self.inner.canonical(id);
        if self.inner.is_folder(&id)? {
            return Err(WriteError::Folder);
        }
        self.remove_any(&id)
    }

    /// Removes the empty folder `id`, as [`Drive::remove`] removes a file.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn remove_folder(&self, id: &NodeId) -> Result<(), WriteError> {
        let id = self.inner.canonical(id);
        if !self.inner.is_folder(&id)? {
            return Err(WriteError::NotFolder);
        }
        if !self.inner.children(&id)?.is_empty() {
            return Err(WriteError::NotEmpty);
        }
        self.remove_any(&id)
    }

    fn remove_any(&self, id: &NodeId) -> Result<(), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let parent = inner.parent_of(id)?.ok_or(WriteError::Gone)?;
        match id {
            NodeId::Top => return Err(WriteError::NotAllowed),
            NodeId::Local(n) => {
                inner.unpair(id);
                inner.forget_made(*n);
            }
            NodeId::Remote(remote) => {
                inner.unpair(id);
                let mut edit = inner.edit_of_remote(remote)?;
                if let Some(dirty) = edit.dirty.take() {
                    let _ = std::fs::remove_file(inner.local_file(dirty.file));
                }
                edit.removed = true;
                edit.awaiting = None;
                edit.session = None;
                edit.refused = None;
                inner.queue(&mut edit);
                inner.put_edit(edit);
            }
        }
        inner.emit(vec![id.clone(), parent]);
        Ok(())
    }

    /// Renames and/or moves `id` to `name` in `folder`. Over a file there, a file gives it its
    /// bytes (a save by rename: the same item on the drive, a new version), and a folder takes
    /// the place of an empty one.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub async fn rename(&self, id: &NodeId, folder: &NodeId, name: &str) -> Result<(), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        if id == NodeId::Top {
            return Err(WriteError::NotAllowed);
        }
        let folder = inner.folder_for(folder)?;
        let node = self.node(&id)?.ok_or(WriteError::Gone)?;
        let is_folder = node.kind == NodeKind::Folder;
        if is_folder && inner.within(&folder, &id) {
            return Err(WriteError::Inside);
        }
        let into_trash = inner.local_only(&folder) && !inner.local_only(&id);
        if into_trash && self.lookup(&folder, name)?.is_some() {
            return Err(WriteError::Exists);
        }
        if into_trash && matches!(id, NodeId::Remote(_)) {
            return self.keep_here(&node, &folder, name).await;
        }
        if let Some(there) = self.lookup(&folder, name)? {
            if there.id == id {
                return Ok(());
            }
            match (is_folder, there.kind == NodeKind::Folder) {
                (true, true) => self.remove_folder(&there.id)?,
                (true, false) => return Err(WriteError::NotFolder),
                (false, true) => return Err(WriteError::Folder),
                (false, false) => return self.save_over(&id, &there.id),
            }
        }
        self.relocate(&node, &folder, name)
    }

    /// Moves `node` to `name` in `folder`, where nothing has that name.
    fn relocate(&self, node: &super::Node, folder: &NodeId, name: &str) -> Result<(), WriteError> {
        let inner = &self.inner;
        let id = node.id.clone();
        let file = node.kind != NodeKind::Folder;
        let (mut edit, kind) = match &id {
            NodeId::Remote(remote) => {
                let kind = inner
                    .entry(remote)?
                    .map_or(ItemKind::File, |entry| entry.kind);
                (inner.edit_of_remote(remote)?, kind)
            }
            local => (inner.edit(local).ok_or(WriteError::Gone)?, ItemKind::File),
        };
        let wanted = naming::drive_name(name, &node.name, &edit.name, &kind);
        if wanted != edit.name || *folder != node.parent {
            inner.check_name(folder, &wanted, file)?;
        }
        let from = node.parent.clone();
        edit.parent = folder.clone();
        edit.name = wanted;
        edit.refused = None;
        if edit.base.is_some() && !edit.pending() && edit.awaiting.is_none() {
            // Back where the drive has it.
            held(&inner.index).edits.remove(&id);
            inner.touched();
        } else {
            inner.put_edit(edit);
        }
        inner.emit(vec![id, from, folder.clone()]);
        Ok(())
    }

    /// Moves the drive's `node` into what stays on this computer, at `name` in `folder`: a copy
    /// of it (and of all in it) is made there, and the drive's items go to its recycle bin.
    async fn keep_here(
        &self,
        node: &super::Node,
        folder: &NodeId,
        name: &str,
    ) -> Result<(), WriteError> {
        let inner = &self.inner;
        let mut made = Vec::new();
        let mut gone = Vec::new();
        let mut moves = Vec::new();
        let copied = self
            .copy_here(node, folder, name, &mut made, &mut gone, &mut moves)
            .await;
        if let Err(why) = copied {
            for copy in made {
                if let NodeId::Local(n) = copy {
                    inner.forget_made(n);
                }
            }
            return Err(why);
        }
        for (id, parent, name) in moves {
            if let Some(mut edit) = inner.edit(&id) {
                edit.parent = parent;
                edit.name = name;
                inner.put_edit(edit);
            }
        }
        // The copy is the item where it went: it keeps its inode number.
        if let Some(copy) = made.first() {
            let ino = inner.ino(&node.id);
            held(&inner.index).inos.insert(key_text(copy), ino);
            held(&inner.inodes).insert(ino, copy.clone());
        }
        for id in gone.iter().rev() {
            if let NodeId::Remote(remote) = id {
                let mut edit = inner.edit_of_remote(remote)?;
                if let Some(dirty) = edit.dirty.take() {
                    let _ = std::fs::remove_file(inner.local_file(dirty.file));
                }
                inner.unpair(id);
                edit.removed = true;
                edit.awaiting = None;
                edit.session = None;
                edit.refused = None;
                inner.queue(&mut edit);
                inner.put_edit(edit);
            }
        }
        let mut changed = vec![node.parent.clone(), folder.clone()];
        changed.extend(made);
        changed.extend(gone);
        inner.emit(changed);
        Ok(())
    }

    /// Copies `node` to `name` in `folder` as an item made here, and what is in it; notes each
    /// copy in `made`, each of the drive's items copied in `gone`, and what was made here inside
    /// it, which moves along once all is copied, in `moves`.
    fn copy_here<'a>(
        &'a self,
        node: &'a super::Node,
        folder: &'a NodeId,
        name: &'a str,
        made: &'a mut Vec<NodeId>,
        gone: &'a mut Vec<NodeId>,
        moves: &'a mut Vec<(NodeId, NodeId, String)>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), WriteError>> + Send + 'a>>
    {
        Box::pin(async move {
            let inner = &self.inner;
            if let NodeId::Local(_) = node.id {
                moves.push((node.id.clone(), folder.clone(), name.to_owned()));
                return Ok(());
            }
            let n = held(&inner.index).take_local();
            let now = now_secs();
            let mut edit = match node.kind {
                NodeKind::Folder => Edit::made_here(n, folder.clone(), name.to_owned(), None, now),
                _ => {
                    let path = inner.local_file(n);
                    new_file(&path)?;
                    let size = match &node.kind {
                        NodeKind::Online { .. } => {
                            let link = self.read(&node.id, 0, node.size).await;
                            let link = link.map_err(WriteError::Read)?;
                            std::fs::write(&path, &link)?;
                            link.len() as u64
                        }
                        _ => match inner.edit(&node.id).and_then(|edit| edit.dirty) {
                            Some(dirty) => {
                                copy_span(&inner.local_file(dirty.file), &path, dirty.size)?;
                                dirty.size
                            }
                            None => {
                                inner
                                    .download_into(self, &node.id, &path, node.size)
                                    .await?;
                                node.size
                            }
                        },
                    };
                    let dirty = Dirty {
                        file: n,
                        size,
                        modified: node.modified.map_or(now, |m| m.0),
                        writes: 1,
                    };
                    inner.wrote(&path);
                    Edit::made_here(n, folder.clone(), name.to_owned(), Some(dirty), now)
                }
            };
            inner.queue(&mut edit);
            let copy = edit.id.clone();
            inner.put_edit(edit);
            made.push(copy.clone());
            gone.push(node.id.clone());
            if node.kind == NodeKind::Folder {
                for child in self.children(&node.id)? {
                    self.copy_here(&child, &copy, &child.name, made, gone, moves)
                        .await?;
                }
            }
            Ok(())
        })
    }

    /// Saves the file `from` over the file `over`: `over` takes its bytes and its place.
    fn save_over(&self, from: &NodeId, over: &NodeId) -> Result<(), WriteError> {
        let inner = &self.inner;
        let source = inner.edit(from);
        let source_parent = inner.parent_of(from)?.ok_or(WriteError::Gone)?;
        let over_parent = inner.parent_of(over)?.ok_or(WriteError::Gone)?;
        match over {
            NodeId::Remote(target) => {
                let bytes = source.as_ref().and_then(|edit| edit.dirty.clone());
                let Some(bytes) = bytes else {
                    // A file of the drive's, unchanged, over another: that one goes to the
                    // recycle bin and this one takes its name.
                    let name = self.node(over)?.map(|n| n.name).ok_or(WriteError::Gone)?;
                    self.remove_any(over)?;
                    let node = self.node(from)?.ok_or(WriteError::Gone)?;
                    return self.relocate(&node, &over_parent, &name);
                };
                let mut edit = inner.edit_of_remote(target)?;
                if let Some(old) = edit.dirty.replace(Dirty {
                    writes: bytes.writes.max(1),
                    ..bytes
                }) {
                    let _ = std::fs::remove_file(inner.local_file(old.file));
                }
                edit.refused = None;
                edit.session = None;
                match from {
                    NodeId::Local(n) => {
                        held(&inner.index).edits.remove(from);
                        inner.put_edit(edit);
                        inner.alias(*n, over);
                    }
                    NodeId::Remote(_) => {
                        // Its bytes went over the other; it goes to the recycle bin.
                        if let Some(mut gone) = source {
                            gone.dirty = None;
                            gone.removed = true;
                            gone.awaiting = None;
                            gone.session = None;
                            inner.queue(&mut gone);
                            held(&inner.index).edits.insert(from.clone(), gone);
                        }
                        let ino = inner.ino(from);
                        held(&inner.index).inos.insert(key_text(over), ino);
                        held(&inner.inodes).insert(ino, over.clone());
                        inner.rekey(from, over);
                        inner.put_edit(edit);
                    }
                    NodeId::Top => return Err(WriteError::NotAllowed),
                }
                inner.touched_write(over);
            }
            NodeId::Local(n) => {
                // Made here and not sent: it is simply replaced.
                let name = self.node(over)?.map(|n| n.name).ok_or(WriteError::Gone)?;
                inner.forget_made(*n);
                held(&inner.index).aliases.insert(*n, from.clone());
                let node = self.node(from)?.ok_or(WriteError::Gone)?;
                self.relocate(&node, &over_parent, &name)?;
            }
            NodeId::Top => return Err(WriteError::NotAllowed),
        }
        inner.wake.notify_one();
        inner.emit(vec![from.clone(), over.clone(), source_parent, over_parent]);
        Ok(())
    }
}
