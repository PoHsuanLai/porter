//! Changes made here: new files and folders, bytes written, sizes, renames and moves (a save by
//! rename over a file among them), and removals. Each is kept in the cache at once and in the
//! index's queue of changes, and sent after (see `send`).
//!
//! - **One owner.** Every change of an item's [`Edit`] is made in one step under the index's
//!   lock, on the edit as it is then ([`Inner::change`]): nothing reads an edit, changes the copy
//!   and puts it back over what changed meanwhile.
//! - **Bytes.** A file changed here is written to `local/<n>`: a copy of the drive's version
//!   made when it is first written (downloaded first when it is not all here), or a new file.
//!   Each change of the bytes takes the file's lock and gives them a new generation, unique in
//!   the index, so an upload of older bytes never counts as one of these. A sync makes the bytes
//!   durable (and the index that names them, and the folder they are in); it does not send them.
//! - **Handles.** A file opened to write is written through its [`WriteHandle`], which follows
//!   the bytes: to the id the drive gives a file made here, to this computer's version when the
//!   drive changed the file meanwhile (see `conflict`), and nowhere once the file was saved over.
//! - **Saving by rename.** A file renamed over a file of the drive's gives that file its bytes:
//!   the same item, a new version, so the drive keeps its history. It is never a removal and a
//!   new file.
//! - **Removals** go to the drive's recycle bin. An item made here and removed before it was sent
//!   is never sent. A file removed while it is open stays usable until it is closed.
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
use crate::index::{Dirty, Edit, Index};
use crate::naming;
use porter_capability::HashKind;
use porter_sync::{ItemKind, RemoteId};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// How many times a change follows bytes that moved under it (sent, or taken into a copy)
/// before it gives up.
const TRIES: usize = 8;

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

/// A file open for writing ([`Drive::open_write`], [`Drive::create`]), closed with
/// [`Drive::release_write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WriteHandle(u64);

/// Keeps `edit` in `index`, unless it holds nothing any more.
fn keep(index: &mut Index, edit: Edit) {
    if !edit.spent() {
        index.edits.insert(edit.id.clone(), edit);
    }
}

/// Copies the first `size` bytes of `from` to `to`.
fn copy_open(from: &std::fs::File, to: &std::fs::File, size: u64) -> std::io::Result<()> {
    let mut at = 0;
    let mut buffer = vec![0; 1 << 20];
    while at < size {
        let len = usize::try_from((size - at).min(buffer.len() as u64)).unwrap_or(0);
        from.read_exact_at(&mut buffer[..len], at)?;
        to.write_all_at(&buffer[..len], at)?;
        at += len as u64;
    }
    to.set_len(size)
}

/// `name` as one plain file name: never empty, never a step up or down a folder.
fn plain_name(name: &str) -> String {
    match name.replace('/', "\u{ff0f}") {
        name if name.is_empty() || name == "." || name == ".." => "Untitled".to_owned(),
        name => name,
    }
}

/// Makes the folder `path` below `into` (and `into`), each folder made synced into the one it
/// is in.
fn make_dirs(into: &Path, path: &Path) -> std::io::Result<()> {
    let mut at = into.to_path_buf();
    let mut steps = vec![at.clone()];
    for part in path.components() {
        at.push(part);
        steps.push(at.clone());
    }
    for step in steps {
        match std::fs::create_dir(&step) {
            Ok(()) => {
                if let Some(parent) = step.parent() {
                    std::fs::File::open(parent)?.sync_all()?;
                }
            }
            Err(why) if why.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&step)?;
            }
            Err(why) => return Err(why),
        }
    }
    Ok(())
}

/// Copies the first `size` bytes of `from` into the file `to`, which exists.
fn copy_span(from: &Path, to: &Path, size: u64) -> Result<(), WriteError> {
    let source = std::fs::File::open(from)?;
    let target = std::fs::OpenOptions::new().write(true).open(to)?;
    copy_open(&source, &target, size)?;
    Ok(())
}

/// The first `size` bytes of `from` as a new owner-only file `to`: a clone sharing its blocks
/// where the file system makes one, else a copy.
pub(super) fn clone_file(from: &Path, to: &Path, size: u64) -> std::io::Result<()> {
    let source = std::fs::File::open(from)?;
    let target = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(to)?;
    match rustix::fs::ioctl_ficlone(&target, &source) {
        Ok(()) => target.set_len(size),
        Err(_) => copy_open(&source, &target, size),
    }
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
            (false, Some(_)) => self.check_path(folder, name, 0),
            _ => Ok(()),
        }
    }

    /// Refuses `name` in `folder` when the path it makes from the top of the drive, with
    /// `below` more characters of what is inside it, is longer than the drive takes.
    fn check_path(&self, folder: &NodeId, name: &str, below: usize) -> Result<(), WriteError> {
        let Some(max) = self.sink.as_ref().and_then(|sink| sink.path_max()) else {
            return Ok(());
        };
        let mut len = name.chars().count() + below;
        let mut at = self.canonical(folder);
        for _ in 0..super::DEPTH {
            if at == NodeId::Top {
                break;
            }
            len += 1 + self.node_name(&at).map_or(0, |name| name.chars().count());
            match self.parent_of(&at) {
                Ok(Some(parent)) => at = parent,
                _ => break,
            }
        }
        match len > max {
            true => Err(WriteError::BadName),
            false => Ok(()),
        }
    }

    /// The most characters a path inside the folder `id` adds to its own: a `/` and a name for
    /// each step down.
    fn deepest_below(&self, id: &NodeId, depth: usize) -> usize {
        if depth >= super::DEPTH {
            return 0;
        }
        self.children(id)
            .unwrap_or_default()
            .iter()
            .map(|child| {
                let name = self
                    .node_name(&child.id)
                    .map_or(0, |name| name.chars().count());
                let inside = match child.kind {
                    NodeKind::Folder => self.deepest_below(&child.id, depth + 1),
                    _ => 0,
                };
                1 + name + inside
            })
            .max()
            .unwrap_or(0)
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

    /// The lock of the bytes `local/<n>`.
    pub(super) fn file_lock(&self, n: u64) -> Arc<Mutex<()>> {
        Arc::clone(held(&self.files).entry(n).or_default())
    }

    /// Removes the bytes `local/<n>`, which nothing names any more.
    pub(super) fn drop_file(&self, n: u64) {
        let _ = std::fs::remove_file(self.local_file(n));
        held(&self.files).remove(&n);
    }

    /// A new, empty, owner-only file `local/<n>`.
    pub(super) fn new_local(&self, n: u64) -> std::io::Result<PathBuf> {
        let path = self.local_file(n);
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        self.local_made.store(true, Ordering::SeqCst);
        Ok(path)
    }

    /// The number of the bytes `id` has here, if any.
    fn dirty_file(&self, id: &NodeId) -> Option<u64> {
        held(&self.index)
            .edits
            .get(id)
            .and_then(|edit| edit.dirty.as_ref())
            .map(|dirty| dirty.file)
    }

    /// Changes the edit of `id` as it is now, in one step under the index's lock; `None` when
    /// it has none. An edit left holding nothing is dropped.
    pub(super) fn change<R>(
        &self,
        id: &NodeId,
        f: impl FnOnce(&mut Edit, &mut Index) -> R,
    ) -> Option<R> {
        let mut index = held(&self.index);
        let mut edit = index.edits.remove(id)?;
        let out = f(&mut edit, &mut index);
        keep(&mut index, edit);
        drop(index);
        self.touched();
        Some(out)
    }

    /// [`Inner::change`] of the drive's item `id`, from the listing's entry when nothing was
    /// changed of it yet. One removed here is [`WriteError::Gone`].
    fn change_remote<R>(
        &self,
        id: &RemoteId,
        f: impl FnOnce(&mut Edit, &mut Index) -> R,
    ) -> Result<R, WriteError> {
        let node = NodeId::Remote(id.clone());
        // Read before the index is locked: the listing has locks of its own.
        let entry = self.listing.entry(id)?;
        let mut index = held(&self.index);
        let mut edit = match index.edits.remove(&node) {
            Some(edit) if edit.removed => {
                index.edits.insert(node, edit);
                return Err(WriteError::Gone);
            }
            Some(edit) => edit,
            None => {
                let mut edit = Edit::of_entry(entry.ok_or(WriteError::Gone)?, now_secs());
                edit.seq = index.take_seq();
                edit
            }
        };
        let out = f(&mut edit, &mut index);
        keep(&mut index, edit);
        drop(index);
        self.touched();
        Ok(out)
    }

    /// Puts an edit made here in the queue, after everything queued before.
    fn put_new(&self, mut edit: Edit) -> NodeId {
        let id = edit.id.clone();
        {
            let mut index = held(&self.index);
            edit.seq = index.take_seq();
            index.edits.insert(id.clone(), edit);
        }
        self.touched();
        self.wake.notify_one();
        id
    }

    /// The upload session of `edit`'s bytes is of no use any more: given up before the next
    /// step is sent.
    pub(super) fn abandon(&self, edit: &mut Edit) {
        if let Some(session) = edit.session.take() {
            held(&self.abandoned).push(session);
        }
        edit.session_of = 0;
    }

    /// `edit`'s bytes changed: they are a new generation, sent afresh.
    fn renew(&self, edit: &mut Edit, index: &mut Index) {
        if let Some(dirty) = edit.dirty.as_mut() {
            dirty.generation = index.take_generation();
            dirty.modified = now_secs();
        }
        self.abandon(edit);
        edit.refused = None;
    }

    /// Opens `id` to write: a new handle.
    fn open_handle(&self, id: &NodeId) -> WriteHandle {
        let handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
        held(&self.handles).insert(handle, Some(id.clone()));
        {
            let mut writing = held(&self.writing);
            let at = writing.entry(id.clone()).or_insert(Writing {
                handles: 0,
                last: Instant::now(),
            });
            at.handles += 1;
            at.last = Instant::now();
        }
        *held(&self.open).entry(id.clone()).or_default() += 1;
        WriteHandle(handle)
    }

    /// What `handle` writes to now.
    pub(super) fn written(&self, handle: WriteHandle) -> Option<NodeId> {
        let id = held(&self.handles).get(&handle.0).cloned().flatten()?;
        Some(self.canonical(&id))
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
        // Removed while open, it is still written until it is closed (never sent).
        let mut removed = false;
        if let Some(edit) = self.edit(&id) {
            if edit.removed && !self.is_open(&id) {
                return Err(WriteError::Gone);
            }
            if edit.dirty.is_some() {
                return Ok(());
            }
            removed = edit.removed;
        }
        let NodeId::Remote(remote) = &id else {
            return Err(WriteError::Gone);
        };
        let entry = self.entry_open(remote)?.ok_or(WriteError::Gone)?;
        match entry.kind {
            ItemKind::Folder => return Err(WriteError::Folder),
            ItemKind::Online { .. } => return Err(WriteError::NotAllowed),
            _ => {}
        }
        let size = keep.unwrap_or(entry.size.0).min(entry.size.0);
        let n = held(&self.index).take_local();
        let path = self.new_local(n)?;
        let cached = held(&self.index)
            .items
            .get(&remote.0)
            .is_some_and(|row| row.version == entry.version.0 && row.present.covers(0, size));
        let copied = match cached {
            true => {
                let (from, to) = (self.content(remote, &entry.version.0), path.clone());
                // A whole file's copy waits on the disk: off the runtime's threads.
                tokio::task::spawn_blocking(move || copy_span(&from, &to, size))
                    .await
                    .unwrap_or(Err(WriteError::Cache))
            }
            false => self.download_into(drive, &id, &path, size).await,
        };
        if let Err(why) = copied {
            self.drop_file(n);
            return Err(why);
        }
        let cut = keep.is_some_and(|k| k != entry.size.0);
        let version = entry.version.0.clone();
        let modified = entry.modified.map_or_else(now_secs, |m| m.0);
        let made_dirty = |edit: &mut Edit, index: &mut Index| {
            // Made dirty meanwhile, or changed against another version since: this copy is not
            // the one to change.
            if edit.dirty.is_some() || edit.base.as_ref().is_none_or(|b| b.version != version) {
                return false;
            }
            edit.dirty = Some(Dirty {
                file: n,
                size,
                modified,
                generation: match cut {
                    true => index.take_generation(),
                    false => 0,
                },
            });
            edit.refused = None;
            true
        };
        let took = match removed {
            true => self.change(&id, made_dirty).ok_or(WriteError::Gone),
            false => self.change_remote(remote, made_dirty),
        };
        match took {
            Ok(true) => {
                self.wrote(&path);
                self.wake.notify_one();
                Ok(())
            }
            Ok(false) => {
                self.drop_file(n);
                Ok(())
            }
            Err(why) => {
                self.drop_file(n);
                Err(why)
            }
        }
    }

    /// Copies the bytes `id` has here into the file `path`, off the runtime's threads and
    /// without their lock (a write waits on it): the copy is kept when they are still the same
    /// generation under the lock after it, else taken again. Their size, or `None` when `id`
    /// has none here.
    async fn copy_written(
        self: &Arc<Self>,
        id: &NodeId,
        path: &Path,
    ) -> Result<Option<u64>, WriteError> {
        for _ in 0..TRIES {
            let Some(dirty) = self.edit(id).and_then(|edit| edit.dirty) else {
                return Ok(None);
            };
            let (inner, id, to) = (Arc::clone(self), id.clone(), path.to_path_buf());
            let kept = tokio::task::spawn_blocking(move || {
                let copied = copy_span(&inner.local_file(dirty.file), &to, dirty.size);
                #[cfg(test)]
                inner.at_hook("copied here");
                let lock = inner.file_lock(dirty.file);
                let _bytes = held(&lock);
                let same = inner
                    .edit(&id)
                    .and_then(|edit| edit.dirty)
                    .is_some_and(|now| {
                        now.file == dirty.file && now.generation == dirty.generation
                    });
                match same {
                    true => copied.map(|()| true),
                    // Written, sent or removed meanwhile: taken again as it is now.
                    false => Ok(false),
                }
            })
            .await
            .unwrap_or(Err(WriteError::Cache))?;
            if kept {
                return Ok(Some(dirty.size));
            }
        }
        Err(WriteError::Gone)
    }

    /// Writes the first `size` bytes of `id` into `path`, read through the cache.
    async fn download_into(
        &self,
        drive: &Drive,
        id: &NodeId,
        path: &Path,
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

    /// Writes `bytes` at `offset` into the bytes `id` has here, under their lock, while they are
    /// what `handle` writes to; `None` when `id` has none here (or they moved under this write).
    fn write_locked(
        &self,
        handle: WriteHandle,
        id: &NodeId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Option<u64>, WriteError> {
        let Some(n) = self.dirty_file(id) else {
            return Ok(None);
        };
        let lock = self.file_lock(n);
        let _bytes = held(&lock);
        if self.written(handle).as_ref() != Some(id) || self.dirty_file(id) != Some(n) {
            return Ok(None);
        }
        let path = self.local_file(n);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|file| file.write_all_at(bytes, offset))?;
        #[cfg(test)]
        self.at_write_hook();
        let end = offset + bytes.len() as u64;
        self.change(id, |edit, index| {
            if let Some(dirty) = edit.dirty.as_mut() {
                dirty.size = dirty.size.max(end);
            }
            self.renew(edit, index);
        });
        self.wrote(&path);
        self.touched_write(id);
        self.wake.notify_one();
        Ok(Some(bytes.len() as u64))
    }

    /// Sets the size of the bytes `id` has here, under their lock; `false` when it has none.
    fn set_len_locked(&self, id: &NodeId, size: u64) -> Result<bool, WriteError> {
        let Some(n) = self.dirty_file(id) else {
            return Ok(false);
        };
        let lock = self.file_lock(n);
        let _bytes = held(&lock);
        if self.dirty_file(id) != Some(n) {
            return Ok(false);
        }
        let path = self.local_file(n);
        let now = held(&self.index)
            .edits
            .get(id)
            .and_then(|edit| edit.dirty.as_ref())
            .map(|dirty| dirty.size);
        if now != Some(size) {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_len(size))?;
            self.change(id, |edit, index| {
                if let Some(dirty) = edit.dirty.as_mut() {
                    dirty.size = size;
                }
                self.renew(edit, index);
            });
            self.wrote(&path);
        }
        self.touched_write(id);
        self.wake.notify_one();
        Ok(true)
    }

    /// Notes that `id` was written or closed now: its quiet time starts again.
    fn touched_write(&self, id: &NodeId) {
        if let Some(writing) = held(&self.writing).get_mut(id) {
            writing.last = Instant::now();
        }
    }

    /// Moves what is known of `from` in memory to `to`, which it became: its handles, its open
    /// count and its quiet time.
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
        for target in held(&self.handles).values_mut() {
            if target.as_ref() == Some(from) {
                *target = Some(to.clone());
            }
        }
    }

    /// Moves only what writes to `from` to `to`: its handles, as many of its opens, and its
    /// quiet time. Who reads `from` still reads it.
    pub(super) fn rekey_writes(&self, from: &NodeId, to: &NodeId) {
        let mut moved = 0;
        for target in held(&self.handles).values_mut() {
            if target.as_ref() == Some(from) {
                *target = Some(to.clone());
                moved += 1;
            }
        }
        if moved == 0 {
            return;
        }
        let writing = held(&self.writing).remove(from);
        if let Some(writing) = writing {
            held(&self.writing).insert(to.clone(), writing);
        }
        let mut open = held(&self.open);
        if let Some(n) = open.get_mut(from) {
            *n = n.saturating_sub(moved);
            if *n == 0 {
                open.remove(from);
            }
        }
        *open.entry(to.clone()).or_default() += moved;
    }

    /// Whether a handle writes to `id` now.
    pub(super) fn written_by_handle(&self, id: &NodeId) -> bool {
        held(&self.handles)
            .values()
            .any(|target| target.as_ref() == Some(id))
    }

    /// The handles writing to `id` write nowhere from now on: what they wrote to was saved
    /// over. Its count of them goes with them.
    fn detach(&self, id: &NodeId) {
        let mut detached = 0;
        for target in held(&self.handles).values_mut() {
            if target.as_ref() == Some(id) {
                *target = None;
                detached += 1;
            }
        }
        if detached == 0 {
            return;
        }
        if let Some(writing) = held(&self.writing).get_mut(id) {
            writing.handles = writing.handles.saturating_sub(detached);
        }
        let mut open = held(&self.open);
        if let Some(n) = open.get_mut(id) {
            *n = n.saturating_sub(detached);
            if *n == 0 {
                open.remove(id);
            }
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
    pub(super) fn forget_made(&self, n: u64) {
        let id = NodeId::Local(n);
        let lock = self.dirty_file(&id).map(|file| self.file_lock(file));
        let _bytes = lock.as_ref().map(|lock| held(lock));
        let gone = held(&self.index).edits.remove(&id);
        if let Some(dirty) = gone.and_then(|edit| edit.dirty) {
            self.drop_file(dirty.file);
        }
        held(&self.writing).remove(&id);
        self.touched();
    }

    /// Nothing has `id` open any more: what was removed while it was open goes now.
    pub(super) fn closed(&self, id: &NodeId) {
        let Some(edit) = self.edit(id).filter(|edit| edit.removed) else {
            return;
        };
        match (&edit.id, edit.dirty) {
            (NodeId::Local(n), _) => self.forget_made(*n),
            (_, Some(dirty)) => {
                let lock = self.file_lock(dirty.file);
                let _bytes = held(&lock);
                let taken = self.change(id, |edit, _| match &edit.dirty {
                    Some(now) if now.file == dirty.file => edit.dirty.take(),
                    _ => None,
                });
                if let Some(Some(dirty)) = taken {
                    self.drop_file(dirty.file);
                }
            }
            _ => {}
        }
    }

    /// `id` was closed for writing by its last handle: bytes copied to change and never written
    /// are dropped, and the file is left as the drive has it.
    fn drop_untouched(&self, id: &NodeId) {
        let Some(edit) = self.edit(id) else {
            return;
        };
        let Some(n) = edit
            .dirty
            .as_ref()
            .filter(|dirty| dirty.generation == 0 && edit.base.is_some())
            .map(|dirty| dirty.file)
        else {
            return;
        };
        let lock = self.file_lock(n);
        let _bytes = held(&lock);
        let taken = self.change(id, |edit, _| match &edit.dirty {
            Some(dirty) if dirty.file == n && dirty.generation == 0 => edit.dirty.take(),
            _ => None,
        });
        if let Some(Some(_)) = taken {
            self.drop_file(n);
            self.emit(vec![id.clone()]);
        }
    }

    /// Marks the drive's item `remote` removed here: it goes to the recycle bin. Its bytes
    /// written here go too, unless `keep_bytes` (it is open, and they go when it is closed).
    fn remove_remote(&self, remote: &RemoteId, keep_bytes: bool) -> Result<(), WriteError> {
        let id = NodeId::Remote(remote.clone());
        let lock = self.dirty_file(&id).map(|file| self.file_lock(file));
        let _bytes = lock.as_ref().map(|lock| held(lock));
        let dropped = self.change_remote(remote, |edit, index| {
            index.unpair(&id);
            edit.removed = true;
            edit.awaiting = None;
            edit.refused = None;
            self.abandon(edit);
            edit.seq = index.take_seq();
            match keep_bytes {
                true => None,
                false => edit.dirty.take(),
            }
        })?;
        if let Some(dirty) = dropped {
            self.drop_file(dirty.file);
        }
        self.wake.notify_one();
        Ok(())
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

impl Drive {
    /// Writes what was made or changed here and not sent yet out as plain files at their paths
    /// below `into` (a folder of their own beside the drive's folder): what removing the account
    /// keeps of it. A file whose place here cannot be told goes at the top under its own name;
    /// lock files are left out, and what stays on this computer (the desktop's trash) is kept
    /// too. Asked again, it writes nothing twice: a file already there with the same content is
    /// left as it is, one with other content takes the next free number. Every file and folder
    /// it writes is synced before it answers. How many files are kept there. The cache itself is
    /// left as it is.
    ///
    /// # Errors
    /// A file or folder could not be written there.
    pub fn keep_unsent(&self, into: &Path) -> std::io::Result<usize> {
        let inner = &self.inner;
        let edits: Vec<Edit> = held(&inner.index).edits.values().cloned().collect();
        let mut kept = 0;
        for edit in edits {
            let made_folder = edit.folder && edit.base.is_none();
            let lock_file = !edit.folder && crate::local_only::is_lock_file(&edit.name);
            if edit.removed || lock_file || (edit.dirty.is_none() && !made_folder) {
                continue;
            }
            let path = self
                .path_here(&edit.id)
                .unwrap_or_else(|| PathBuf::from(plain_name(&edit.name)));
            let Some(dirty) = &edit.dirty else {
                make_dirs(into, &path)?;
                continue;
            };
            let folder = path.parent().map_or_else(PathBuf::new, Path::to_path_buf);
            make_dirs(into, &folder)?;
            let lock = inner.file_lock(dirty.file);
            let _bytes = held(&lock);
            let from = inner.local_file(dirty.file);
            let ours = crate::hash::of_file(HashKind::Sha256, &from)?;
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let at = into.join(&folder);
            let mut target = None;
            for n in 1.. {
                let candidate = at.join(match n {
                    1 => name.clone(),
                    n => naming::numbered(&name, n),
                });
                if candidate.symlink_metadata().is_err() {
                    target = Some(candidate);
                    break;
                }
                if ours.is_some() && crate::hash::of_file(HashKind::Sha256, &candidate)? == ours {
                    break;
                }
            }
            kept += 1;
            let Some(to) = target else {
                // Kept already, by an earlier try.
                continue;
            };
            let source = std::fs::File::open(&from)?;
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&to)?;
            copy_open(&source, &written, dirty.size)?;
            written.sync_all()?;
            std::fs::File::open(&at)?.sync_all()?;
        }
        Ok(kept)
    }

    /// Where `id` is shown, below the drive's folder: its names from the top.
    fn path_here(&self, id: &NodeId) -> Option<PathBuf> {
        let mut names = Vec::new();
        let mut at = self.inner.canonical(id);
        for _ in 0..super::DEPTH {
            if at == NodeId::Top {
                names.reverse();
                return Some(names.iter().collect());
            }
            let node = self.node(&at).ok().flatten()?;
            if node.name.is_empty() || node.name == "." || node.name == ".." {
                return None;
            }
            names.push(node.name);
            at = self.inner.parent_of(&at).ok().flatten()?;
        }
        None
    }

    /// Makes an empty file `name` in `folder`, open for writing through the handle it answers
    /// with (as [`Drive::open_write`] opens; close it with [`Drive::release_write`]).
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn create(&self, folder: &NodeId, name: &str) -> Result<(NodeId, WriteHandle), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let folder = inner.folder_for(folder)?;
        inner.check_name(&folder, name, true)?;
        if self.lookup(&folder, name)?.is_some() {
            return Err(WriteError::Exists);
        }
        let n = held(&inner.index).take_local();
        let path = inner.new_local(n)?;
        let now = now_secs();
        let generation = held(&inner.index).take_generation();
        let dirty = Dirty {
            file: n,
            size: 0,
            modified: now,
            generation,
        };
        let edit = Edit::made_here(n, folder.clone(), name.to_owned(), Some(dirty), now);
        let id = edit.id.clone();
        let handle = inner.open_handle(&id);
        inner.wrote(&path);
        inner.put_new(edit);
        inner.ino(&id);
        inner.emit(vec![id.clone(), folder]);
        Ok((id, handle))
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
        let edit = Edit::made_here(n, folder.clone(), name.to_owned(), None, now_secs());
        let id = inner.put_new(edit);
        inner.ino(&id);
        inner.emit(vec![id.clone(), folder]);
        Ok(id)
    }

    /// Opens the file `id` for writing (`truncate`: emptied), as many times as it is asked: a
    /// handle each, closed with [`Drive::release_write`]. Nothing is copied until it is written
    /// (then the drive's version is, downloaded first when it is not all here); it is not
    /// evicted while open.
    ///
    /// # Errors
    /// See [`WriteError`]: an online document's content is never written.
    pub async fn open_write(&self, id: &NodeId, truncate: bool) -> Result<WriteHandle, WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        match inner.attrs(&id)?.map(|attrs| attrs.kind) {
            None => return Err(WriteError::Gone),
            Some(NodeKind::Folder) => return Err(WriteError::Folder),
            Some(NodeKind::Online { .. }) => return Err(WriteError::NotAllowed),
            Some(NodeKind::File) => {}
        }
        let handle = inner.open_handle(&id);
        if truncate && let Err(why) = self.set_len(&id, 0).await {
            self.release_write(handle);
            return Err(why);
        }
        Ok(handle)
    }

    /// What the handle writes to now: the file it opened, as the drive names it, or this
    /// computer's version of it (see [`WriteHandle`]); `None` once it writes nowhere.
    pub fn written(&self, handle: WriteHandle) -> Option<NodeId> {
        self.inner.written(handle)
    }

    /// Writes `bytes` at `offset` into the file open as `handle`; how many were written.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub async fn write(
        &self,
        handle: WriteHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<u64, WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        for _ in 0..TRIES {
            let id = inner.written(handle).ok_or(WriteError::Gone)?;
            match inner.write_locked(handle, &id, offset, bytes)? {
                Some(n) => {
                    inner.emit(vec![id]);
                    return Ok(n);
                }
                // Its bytes are copied to change, unless the handle went elsewhere meanwhile.
                None if inner.written(handle).as_ref() == Some(&id) => {
                    inner.make_dirty(self, &id, None).await?;
                }
                None => {}
            }
        }
        Err(WriteError::Gone)
    }

    /// Sets the size of the file `id`: cut, or grown with zeros.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub async fn set_len(&self, id: &NodeId, size: u64) -> Result<(), WriteError> {
        let inner = &self.inner;
        inner.writable()?;
        let id = inner.canonical(id);
        for _ in 0..TRIES {
            match inner.set_len_locked(&id, size)? {
                true => {
                    inner.emit(vec![id]);
                    return Ok(());
                }
                false => inner.make_dirty(self, &id, Some(size)).await?,
            }
        }
        Err(WriteError::Gone)
    }

    /// Sets the size of the file open as `handle`, as [`Drive::set_len`].
    ///
    /// # Errors
    /// See [`WriteError`].
    pub async fn truncate(&self, handle: WriteHandle, size: u64) -> Result<(), WriteError> {
        let id = self.inner.written(handle).ok_or(WriteError::Gone)?;
        self.set_len(&id, size).await
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
        let kept = inner.change(&id, |edit, _| match edit.dirty.as_mut() {
            Some(dirty) => {
                dirty.modified = at;
                true
            }
            None => false,
        });
        if kept == Some(true) {
            inner.emit(vec![id]);
            return Ok(());
        }
        match inner.attrs(&id)? {
            Some(_) => Ok(()),
            None => Err(WriteError::Gone),
        }
    }

    /// Makes what was written to `id` durable here, with the index that names it and the folder
    /// its bytes are in. It is not sent any sooner.
    ///
    /// # Errors
    /// See [`WriteError`].
    pub fn sync(&self, id: &NodeId) -> Result<(), WriteError> {
        let inner = &self.inner;
        let id = inner.canonical(id);
        if let Some(n) = inner.dirty_file(&id) {
            match std::fs::File::open(inner.local_file(n)).and_then(|f| f.sync_data()) {
                // Sent meanwhile: the bytes are the cache's, which the index's write syncs.
                Err(why) if why.kind() == std::io::ErrorKind::NotFound => {}
                other => other?,
            }
        }
        inner.touched();
        inner.flush_now()?;
        Ok(())
    }

    /// Closes what [`Drive::open_write`] or [`Drive::create`] opened. Once nothing has it open,
    /// it is sent after the quiet time; a file opened and closed with nothing written is left as
    /// the drive has it.
    pub fn release_write(&self, handle: WriteHandle) {
        let inner = &self.inner;
        let Some(target) = held(&inner.handles).remove(&handle.0) else {
            return;
        };
        // A handle whose file was saved over had its counts taken with it.
        let Some(id) = target.map(|target| inner.canonical(&target)) else {
            return;
        };
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
        if closed {
            inner.drop_untouched(&id);
        }
        inner.wake.notify_one();
        self.release(&id);
    }

    /// Removes the file `id`: an item of the drive's goes to its recycle bin, one made here and
    /// not sent yet is gone. Open, it stays usable until it is closed.
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
        let open = inner.is_open(id);
        match id {
            NodeId::Top => return Err(WriteError::NotAllowed),
            NodeId::Local(n) => {
                held(&inner.index).unpair(id);
                match open {
                    // Kept, unseen and never sent, until it is closed.
                    true => {
                        inner.change(id, |edit, _| edit.removed = true);
                    }
                    false => inner.forget_made(*n),
                }
            }
            NodeId::Remote(remote) => inner.remove_remote(remote, open)?,
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
        let kind = match &id {
            NodeId::Remote(remote) => inner
                .entry(remote)?
                .map_or(ItemKind::File, |entry| entry.kind),
            _ => ItemKind::File,
        };
        let now = inner.node_name(&id).ok_or(WriteError::Gone)?;
        let wanted = naming::drive_name(name, &node.name, &now, &kind);
        if wanted != now || *folder != node.parent {
            inner.check_name(folder, &wanted, file)?;
            // What is inside a folder moves with it, at paths longer by as much.
            if !file && !inner.local_only(folder) {
                inner.check_path(folder, &wanted, inner.deepest_below(&id, 0))?;
            }
        }
        let place = |edit: &mut Edit, _: &mut Index| {
            edit.parent = folder.clone();
            edit.name = wanted.clone();
            edit.refused = None;
        };
        match &id {
            NodeId::Remote(remote) => inner.change_remote(remote, place)?,
            local => inner.change(local, place).ok_or(WriteError::Gone)?,
        }
        inner.wake.notify_one();
        inner.emit(vec![id, node.parent.clone(), folder.clone()]);
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
            inner.change(&id, |edit, _| {
                edit.parent = parent;
                edit.name = name;
            });
        }
        // The copy is the item where it went: it keeps its inode number.
        if let Some(copy) = made.first() {
            let ino = inner.ino(&node.id);
            held(&inner.index).inos.insert(key_text(copy), ino);
            held(&inner.inodes).insert(ino, copy.clone());
        }
        for id in gone.iter().rev() {
            if let NodeId::Remote(remote) = id {
                inner.remove_remote(remote, false)?;
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
            let edit = match node.kind {
                NodeKind::Folder => Edit::made_here(n, folder.clone(), name.to_owned(), None, now),
                _ => {
                    let path = inner.new_local(n)?;
                    let size = match &node.kind {
                        NodeKind::Online { .. } => {
                            let link = self.read(&node.id, 0, node.size).await;
                            let link = link.map_err(WriteError::Read)?;
                            std::fs::write(&path, &link)?;
                            link.len() as u64
                        }
                        _ => match inner.copy_written(&node.id, &path).await? {
                            Some(size) => size,
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
                        generation: held(&inner.index).take_generation(),
                    };
                    inner.wrote(&path);
                    Edit::made_here(n, folder.clone(), name.to_owned(), Some(dirty), now)
                }
            };
            let copy = inner.put_new(edit);
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

    /// Saves the file `from` over the file `over`: `over` takes its bytes (a new generation of
    /// them) and its place. What was open on `over` writes nowhere from now on.
    fn save_over(&self, from: &NodeId, over: &NodeId) -> Result<(), WriteError> {
        let inner = &self.inner;
        let source_parent = inner.parent_of(from)?.ok_or(WriteError::Gone)?;
        let over_parent = inner.parent_of(over)?.ok_or(WriteError::Gone)?;
        match over {
            NodeId::Remote(target) => {
                let Some(file) = inner.dirty_file(from) else {
                    // A file of the drive's, unchanged, over another: that one goes to the
                    // recycle bin and this one takes its name.
                    let name = self.node(over)?.map(|n| n.name).ok_or(WriteError::Gone)?;
                    inner.detach(over);
                    self.remove_any(over)?;
                    let node = self.node(from)?.ok_or(WriteError::Gone)?;
                    return self.relocate(&node, &over_parent, &name);
                };
                // Both files' bytes change hands: both locks, the lower number first.
                let mut files: Vec<u64> = std::iter::once(file)
                    .chain(inner.dirty_file(over))
                    .collect();
                files.sort_unstable();
                files.dedup();
                let locks: Vec<_> = files.iter().map(|n| inner.file_lock(*n)).collect();
                let guards: Vec<_> = locks.iter().map(|lock| held(lock)).collect();
                let replaced = inner.change_remote(target, |edit, index| {
                    let bytes = index
                        .edits
                        .get(from)
                        .and_then(|source| source.dirty.clone())
                        .filter(|bytes| bytes.file == file)?;
                    let old = edit.dirty.replace(Dirty {
                        generation: index.take_generation(),
                        ..bytes
                    });
                    self.inner.abandon(edit);
                    edit.refused = None;
                    match from {
                        NodeId::Remote(_) => {
                            // Its bytes went over the other; it goes to the recycle bin.
                            let seq = index.take_seq();
                            if let Some(gone) = index.edits.get_mut(from) {
                                gone.dirty = None;
                                gone.removed = true;
                                gone.awaiting = None;
                                self.inner.abandon(gone);
                                gone.seq = seq;
                            }
                        }
                        _ => {
                            index.edits.remove(from);
                        }
                    }
                    Some(old)
                })?;
                let Some(old) = replaced else {
                    return Err(WriteError::Gone);
                };
                if let Some(old) = old.filter(|old| old.file != file) {
                    inner.drop_file(old.file);
                }
                // Before the bytes' locks go: no write of a handle of `over` gets in between.
                inner.detach(over);
                #[cfg(test)]
                inner.at_hook("saved over");
                drop(guards);
                match from {
                    NodeId::Local(n) => inner.alias(*n, over),
                    _ => {
                        let ino = inner.ino(from);
                        held(&inner.index).inos.insert(key_text(over), ino);
                        held(&inner.inodes).insert(ino, over.clone());
                        inner.rekey(from, over);
                    }
                }
                inner.touched_write(over);
            }
            NodeId::Local(n) => {
                // Made here and not sent: it is simply replaced.
                let name = self.node(over)?.map(|n| n.name).ok_or(WriteError::Gone)?;
                inner.detach(over);
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
