//! fuser's calls over a drive.

use fuser::{
    BsdFileFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, InitFlags, KernelConfig, LockOwner, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs,
    ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use porter_drive::{Drive, NodeId, NodeKind, ROOT_INO, ReadError, WriteError, WriteHandle};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long the kernel may keep an entry or attributes before asking again: short, since the
/// listing changes under it.
const TTL: Duration = Duration::from_secs(1);

/// The block size reported.
const BLOCK: u32 = 4096;

/// A node's attributes as the file system reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    /// Its inode number.
    pub ino: u64,
    /// Whether it is a folder.
    pub folder: bool,
    /// Its size in bytes.
    pub size: u64,
    /// Its mtime, seconds since the epoch.
    pub mtime: i64,
    /// Whether its content may be written (an online document's link file's may not).
    pub writable: bool,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Its inode number.
    pub ino: u64,
    /// Whether it is a folder.
    pub folder: bool,
    /// Its name.
    pub name: String,
}

/// What `statfs` reports, in blocks of [`BLOCK`] bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// All of it.
    pub blocks: u64,
    /// What is free.
    pub free: u64,
}

/// The errno a failed read answers.
pub fn errno_of(error: ReadError) -> Errno {
    match error {
        ReadError::Offline => Errno::ENETDOWN,
        ReadError::Gone => Errno::ENOENT,
        ReadError::Folder => Errno::EISDIR,
        _ => Errno::EIO,
    }
}

/// The errno a refused change answers.
pub fn write_errno(error: WriteError) -> Errno {
    match error {
        WriteError::ReadOnly => Errno::EROFS,
        WriteError::NotAllowed => Errno::EPERM,
        WriteError::BadName | WriteError::Inside => Errno::EINVAL,
        WriteError::Exists => Errno::EEXIST,
        WriteError::Gone => Errno::ENOENT,
        WriteError::NotEmpty => Errno::ENOTEMPTY,
        WriteError::Folder => Errno::EISDIR,
        WriteError::NotFolder => Errno::ENOTDIR,
        WriteError::Read(error) => errno_of(error),
        _ => Errno::EIO,
    }
}

/// The kind bits of a `mode`.
const KIND: u32 = 0o170_000;

/// A regular file's kind bits.
const REGULAR: u32 = 0o100_000;

/// One open file: which, and the drive's handle when it was opened to write.
#[derive(Debug, Clone)]
struct Handle {
    id: NodeId,
    write: Option<WriteHandle>,
}

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What time it is, for a `touch` that names none: the host's clock, since the file system
/// reads none.
#[derive(Clone)]
pub struct Now(std::sync::Arc<dyn Fn() -> SystemTime + Send + Sync>);

impl Now {
    /// The time as `read` says it.
    pub fn new(read: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(read))
    }
}

impl std::fmt::Debug for Now {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Now")
    }
}

/// One drive as a file system: read-only over a drive opened so, else writable.
#[derive(Debug)]
pub struct DriveFs {
    drive: Drive,
    runtime: tokio::runtime::Handle,
    now: Now,
    uid: u32,
    gid: u32,
    handles: Mutex<HashMap<u64, Handle>>,
    next: AtomicU64,
}

impl DriveFs {
    /// The file system over `drive`, its waiting reads on `runtime`, its files owned by this
    /// process's user, a `touch` with no time taking it from `now`.
    pub fn new(drive: Drive, runtime: tokio::runtime::Handle, now: Now) -> Self {
        Self {
            drive,
            runtime,
            now,
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
            handles: Mutex::default(),
            next: AtomicU64::new(1),
        }
    }

    /// Whether the drive takes changes.
    pub fn writable(&self) -> bool {
        self.drive.writable()
    }

    fn handle(&self, id: NodeId, write: Option<WriteHandle>) -> u64 {
        let fh = self.next.fetch_add(1, Ordering::Relaxed);
        held(&self.handles).insert(fh, Handle { id, write });
        fh
    }

    fn named<'a>(&self, parent: u64, name: &'a OsStr) -> Result<(NodeId, &'a str), Errno> {
        let folder = self.node_of(parent)?;
        let name = name.to_str().ok_or(Errno::EINVAL)?;
        Ok((folder, name))
    }

    /// The node `name` in `parent`.
    fn child(&self, parent: u64, name: &OsStr) -> Result<NodeId, Errno> {
        let (folder, name) = self.named(parent, name)?;
        self.drive
            .lookup(&folder, name)
            .map_err(|_| Errno::EIO)?
            .map(|node| node.id)
            .ok_or(Errno::ENOENT)
    }

    fn node_of(&self, ino: u64) -> Result<NodeId, Errno> {
        match ino {
            ROOT_INO => Ok(NodeId::Top),
            ino => self.drive.by_ino(ino).ok_or(Errno::ENOENT),
        }
    }

    fn attr_of(&self, id: &NodeId) -> Result<Attr, Errno> {
        let attrs = self
            .drive
            .attrs(id)
            .map_err(|_| Errno::EIO)?
            .ok_or(Errno::ENOENT)?;
        Ok(Attr {
            ino: self.drive.ino(id),
            folder: attrs.kind == NodeKind::Folder,
            size: attrs.size,
            mtime: attrs.modified.map_or(0, |m| m.0),
            writable: !matches!(attrs.kind, NodeKind::Online { .. }),
        })
    }

    /// `name` in the folder `parent`.
    ///
    /// # Errors
    /// `ENOENT` when it is not there, `EIO` when the listing could not be read.
    pub fn lookup(&self, parent: u64, name: &OsStr) -> Result<Attr, Errno> {
        let folder = self.node_of(parent)?;
        let name = name.to_str().ok_or(Errno::ENOENT)?;
        let node = self
            .drive
            .lookup(&folder, name)
            .map_err(|_| Errno::EIO)?
            .ok_or(Errno::ENOENT)?;
        self.attr_of(&node.id)
    }

    /// The attributes of `ino`.
    ///
    /// # Errors
    /// `ENOENT` when it is not there.
    pub fn getattr(&self, ino: u64) -> Result<Attr, Errno> {
        let id = self.node_of(ino)?;
        self.attr_of(&id)
    }

    /// The entries of the folder `ino`, `.` and `..` first.
    ///
    /// # Errors
    /// `ENOTDIR` for a file, `ENOENT` when it is not there.
    pub fn readdir(&self, ino: u64) -> Result<Vec<DirEntry>, Errno> {
        let id = self.node_of(ino)?;
        let attr = self.attr_of(&id)?;
        if !attr.folder {
            return Err(Errno::ENOTDIR);
        }
        let parent = match &id {
            NodeId::Top => ROOT_INO,
            other => self
                .drive
                .node(other)
                .ok()
                .flatten()
                .map_or(ROOT_INO, |node| self.drive.ino(&node.parent)),
        };
        let mut entries = vec![
            DirEntry {
                ino,
                folder: true,
                name: ".".into(),
            },
            DirEntry {
                ino: parent,
                folder: true,
                name: "..".into(),
            },
        ];
        for node in self.drive.children(&id).map_err(|_| Errno::EIO)? {
            entries.push(DirEntry {
                ino: self.drive.ino(&node.id),
                folder: node.kind == NodeKind::Folder,
                name: node.name,
            });
        }
        Ok(entries)
    }

    /// Opens `ino`: to read, a file not here starts downloading; to write (`truncate`: emptied
    /// first), the drive's version is copied to change, downloaded first when it is not all
    /// here.
    ///
    /// # Errors
    /// `EROFS` to write a read-only drive, `EPERM` to write an online document's link file,
    /// `EISDIR` for a folder, `ENOENT` when it is not there.
    pub async fn open(&self, ino: u64, write: bool, truncate: bool) -> Result<u64, Errno> {
        let id = self.node_of(ino)?;
        let write = match write {
            true => Some(
                self.drive
                    .open_write(&id, truncate)
                    .await
                    .map_err(write_errno)?,
            ),
            false => {
                self.drive.open_file(&id).map_err(errno_of)?;
                None
            }
        };
        Ok(self.handle(id, write))
    }

    /// Makes the file `name` in `parent`, open to write: its attributes and handle. Only a
    /// regular file is made (`mode`'s kind; none is a regular file).
    ///
    /// # Errors
    /// `EPERM` for any other kind, `EEXIST` when the name is taken, `EINVAL` for a name the
    /// drive refuses, `EROFS` on a read-only drive.
    pub fn create(&self, parent: u64, name: &OsStr, mode: u32) -> Result<(Attr, u64), Errno> {
        if !matches!(mode & KIND, 0 | REGULAR) {
            return Err(Errno::EPERM);
        }
        let (folder, name) = self.named(parent, name)?;
        let (id, write) = self.drive.create(&folder, name).map_err(write_errno)?;
        let fh = self.handle(id.clone(), Some(write));
        match self.attr_of(&id) {
            Ok(attr) => Ok((attr, fh)),
            Err(errno) => {
                self.release(fh);
                Err(errno)
            }
        }
    }

    /// Makes the file `name` in `parent`, closed, as `mknod` does for a regular file.
    ///
    /// # Errors
    /// As [`DriveFs::create`].
    pub fn mknod(&self, parent: u64, name: &OsStr, mode: u32) -> Result<Attr, Errno> {
        let (attr, fh) = self.create(parent, name, mode)?;
        self.release(fh);
        Ok(attr)
    }

    /// Makes the folder `name` in `parent`.
    ///
    /// # Errors
    /// As [`DriveFs::create`].
    pub fn mkdir(&self, parent: u64, name: &OsStr) -> Result<Attr, Errno> {
        let (folder, name) = self.named(parent, name)?;
        let id = self.drive.make_folder(&folder, name).map_err(write_errno)?;
        self.attr_of(&id)
    }

    /// Writes `data` at `offset` into the file open as `fh`; how many bytes were written.
    ///
    /// # Errors
    /// `EBADF` for a handle not open to write.
    pub async fn write(&self, fh: u64, offset: u64, data: &[u8]) -> Result<u32, Errno> {
        let handle = held(&self.handles).get(&fh).cloned().ok_or(Errno::EBADF)?;
        let write = handle.write.ok_or(Errno::EBADF)?;
        let n = self
            .drive
            .write(write, offset, data)
            .await
            .map_err(write_errno)?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// Sets `ino`'s size and when it last changed, as `truncate` and `utimes` do (`fh`: through
    /// the file open so, as `ftruncate` does); a mode or an owner is taken and ignored (a drive
    /// keeps neither). Its attributes after.
    ///
    /// # Errors
    /// As [`DriveFs::open`] to write.
    pub async fn setattr(
        &self,
        ino: u64,
        fh: Option<u64>,
        size: Option<u64>,
        modified: Option<SystemTime>,
    ) -> Result<Attr, Errno> {
        let write = fh.and_then(|fh| held(&self.handles).get(&fh).and_then(|h| h.write));
        // Through its handle, the file it writes to now (this computer's version of it, after a
        // conflict).
        let id = match write.and_then(|write| self.drive.written(write)) {
            Some(id) => id,
            None => self.node_of(ino)?,
        };
        if let Some(size) = size {
            match write {
                Some(write) => self.drive.truncate(write, size).await,
                None => self.drive.set_len(&id, size).await,
            }
            .map_err(write_errno)?;
        }
        if let Some(at) = modified {
            let secs = at
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
            self.drive.set_modified(&id, secs).map_err(write_errno)?;
        }
        self.attr_of(&id)
    }

    /// Renames `name` in `parent` to `new_name` in `new_parent`, over a file there when
    /// `replace`.
    ///
    /// # Errors
    /// `EEXIST` when the new name is taken and not to be replaced, else as the core refuses.
    pub async fn rename(
        &self,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        replace: bool,
    ) -> Result<(), Errno> {
        let id = self.child(parent, name)?;
        let (folder, new_name) = self.named(new_parent, new_name)?;
        if !replace
            && self
                .drive
                .lookup(&folder, new_name)
                .map_err(|_| Errno::EIO)?
                .is_some()
        {
            return Err(Errno::EEXIST);
        }
        self.drive
            .rename(&id, &folder, new_name)
            .await
            .map_err(write_errno)
    }

    /// Removes the file `name` in `parent`: to the drive's recycle bin.
    ///
    /// # Errors
    /// `EISDIR` for a folder, `ENOENT` when it is not there.
    pub fn unlink(&self, parent: u64, name: &OsStr) -> Result<(), Errno> {
        let id = self.child(parent, name)?;
        self.drive.remove(&id).map_err(write_errno)
    }

    /// Removes the empty folder `name` in `parent`: to the drive's recycle bin.
    ///
    /// # Errors
    /// `ENOTEMPTY` when it is not empty, `ENOTDIR` for a file.
    pub fn rmdir(&self, parent: u64, name: &OsStr) -> Result<(), Errno> {
        let id = self.child(parent, name)?;
        self.drive.remove_folder(&id).map_err(write_errno)
    }

    /// Makes what was written through `fh` durable here (it is not sent any sooner).
    ///
    /// # Errors
    /// `EBADF` for a handle not open, `EIO` when the cache could not be written.
    pub fn fsync(&self, fh: u64) -> Result<(), Errno> {
        let id = self.target(fh)?;
        self.drive.sync(&id).map_err(write_errno)
    }

    /// `size` bytes of the file open as `fh` from `offset`, waiting for them when they are not
    /// here yet.
    ///
    /// # Errors
    /// `EBADF` for a handle not open, else as [`errno_of`] says.
    pub async fn read(&self, fh: u64, offset: u64, size: u32) -> Result<Vec<u8>, Errno> {
        let id = self.target(fh)?;
        self.drive
            .read(&id, offset, u64::from(size))
            .await
            .map_err(errno_of)
    }

    /// What `fh` reads and writes now: for a handle open to write, the file the drive says it
    /// writes to (this computer's version, after a conflict), else the file it opened.
    fn target(&self, fh: u64) -> Result<NodeId, Errno> {
        let handle = held(&self.handles).get(&fh).cloned().ok_or(Errno::EBADF)?;
        Ok(handle
            .write
            .and_then(|write| self.drive.written(write))
            .unwrap_or(handle.id))
    }

    /// Closes `fh`: a file written through it is sent once it has been left alone.
    pub fn release(&self, fh: u64) {
        match held(&self.handles).remove(&fh) {
            Some(Handle {
                write: Some(write), ..
            }) => self.drive.release_write(write),
            Some(Handle { id, write: None }) => self.drive.release(&id),
            None => {}
        }
    }

    /// The account's space as the drive last said, else nothing known.
    pub fn statfs(&self) -> Space {
        match self.drive.quota() {
            Some((used, Some(total))) => Space {
                blocks: total / u64::from(BLOCK),
                free: total.saturating_sub(used) / u64::from(BLOCK),
            },
            _ => Space { blocks: 0, free: 0 },
        }
    }

    fn perm(&self, attr: &Attr) -> u16 {
        match (attr.folder, self.drive.writable() && attr.writable) {
            (true, true) => 0o755,
            (true, false) => 0o555,
            (false, true) => 0o644,
            (false, false) => 0o444,
        }
    }

    fn file_attr(&self, attr: &Attr) -> FileAttr {
        let mtime = UNIX_EPOCH + Duration::from_secs(u64::try_from(attr.mtime).unwrap_or(0));
        FileAttr {
            ino: INodeNo(attr.ino),
            size: attr.size,
            blocks: attr.size.div_ceil(512),
            atime: mtime,
            mtime,
            ctime: mtime,
            crtime: mtime,
            kind: kind_of(attr.folder),
            perm: self.perm(attr),
            nlink: if attr.folder { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: BLOCK,
            flags: 0,
        }
    }
}

fn kind_of(folder: bool) -> FileType {
    match folder {
        true => FileType::Directory,
        false => FileType::RegularFile,
    }
}

/// The mount, sharing the file system with the reads it runs on the runtime.
#[derive(Debug)]
pub(crate) struct Mounted(pub(crate) std::sync::Arc<DriveFs>);

impl Filesystem for Mounted {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        // An open that empties the file says so, rather than a separate truncate first: nothing
        // is downloaded to be cut away.
        let _ = config.add_capabilities(InitFlags::FUSE_ATOMIC_O_TRUNC);
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.0.lookup(parent.0, name) {
            Ok(attr) => reply.entry(&TTL, &self.0.file_attr(&attr), Generation(0)),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.0.getattr(ino.0) {
            Ok(attr) => reply.attr(&TTL, &self.0.file_attr(&attr)),
            Err(errno) => reply.error(errno),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        match self.0.readdir(ino.0) {
            Ok(entries) => {
                let from = usize::try_from(offset).unwrap_or(usize::MAX);
                for (i, entry) in entries.iter().enumerate().skip(from) {
                    let next = u64::try_from(i + 1).unwrap_or(u64::MAX);
                    if reply.add(INodeNo(entry.ino), next, kind_of(entry.folder), &entry.name) {
                        break;
                    }
                }
                reply.ok();
            }
            Err(errno) => reply.error(errno),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let write = flags.acc_mode() != OpenAccMode::O_RDONLY;
        let truncate = rustix::fs::OFlags::from_bits_retain(flags.0 as u32)
            .contains(rustix::fs::OFlags::TRUNC);
        let fs = std::sync::Arc::clone(&self.0);
        self.0.runtime.spawn(async move {
            match fs.open(ino.0, write, truncate).await {
                Ok(fh) => reply.opened(FileHandle(fh), FopenFlags::empty()),
                Err(errno) => reply.error(errno),
            }
        });
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        match self.0.create(parent.0, name, mode) {
            Ok((attr, fh)) => reply.created(
                &TTL,
                &self.0.file_attr(&attr),
                Generation(0),
                FileHandle(fh),
                FopenFlags::empty(),
            ),
            Err(errno) => reply.error(errno),
        }
    }

    fn mknod(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        match self.0.mknod(parent.0, name, mode) {
            Ok(attr) => reply.entry(&TTL, &self.0.file_attr(&attr), Generation(0)),
            Err(errno) => reply.error(errno),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        match self.0.mkdir(parent.0, name) {
            Ok(attr) => reply.entry(&TTL, &self.0.file_attr(&attr), Generation(0)),
            Err(errno) => reply.error(errno),
        }
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let fs = std::sync::Arc::clone(&self.0);
        let data = data.to_vec();
        self.0.runtime.spawn(async move {
            match fs.write(fh.0, offset, &data).await {
                Ok(n) => reply.written(n),
                Err(errno) => reply.error(errno),
            }
        });
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let modified = mtime.map(|at| match at {
            TimeOrNow::SpecificTime(at) => at,
            TimeOrNow::Now => (self.0.now.0)(),
        });
        let fs = std::sync::Arc::clone(&self.0);
        self.0.runtime.spawn(async move {
            match fs.setattr(ino.0, fh.map(|fh| fh.0), size, modified).await {
                Ok(attr) => reply.attr(&TTL, &fs.file_attr(&attr)),
                Err(errno) => reply.error(errno),
            }
        });
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        if flags.contains(RenameFlags::RENAME_EXCHANGE) {
            reply.error(Errno::EINVAL);
            return;
        }
        let replace = !flags.contains(RenameFlags::RENAME_NOREPLACE);
        let fs = std::sync::Arc::clone(&self.0);
        let (name, newname) = (name.to_owned(), newname.to_owned());
        self.0.runtime.spawn(async move {
            match fs
                .rename(parent.0, &name, newparent.0, &newname, replace)
                .await
            {
                Ok(()) => reply.ok(),
                Err(errno) => reply.error(errno),
            }
        });
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.0.unlink(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        match self.0.rmdir(parent.0, name) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn symlink(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _link_name: &OsStr,
        _target: &Path,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EPERM);
    }

    fn link(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _newparent: INodeNo,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EPERM);
    }

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        // The bytes are in the cache already; they are sent once the file is closed.
        reply.ok();
    }

    fn fsync(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.0.fsync(fh.0) {
            Ok(()) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let fs = std::sync::Arc::clone(&self.0);
        self.0.runtime.spawn(async move {
            match fs.read(fh.0, offset, size).await {
                Ok(bytes) => reply.data(&bytes),
                Err(errno) => reply.error(errno),
            }
        });
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.0.release(fh.0);
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let space = self.0.statfs();
        reply.statfs(
            space.blocks,
            space.free,
            space.free,
            0,
            0,
            BLOCK,
            255,
            BLOCK,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_drive::testing::{MemoryListing, MemorySink, MemorySource, Took};
    use porter_drive::{DriveConfig, Reason};
    use std::sync::Arc;

    async fn fs(dir: &test_kit::Scratch) -> (Arc<MemoryListing>, Arc<MemorySource>, DriveFs) {
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        let drive = Drive::open(
            DriveConfig {
                chunk: 4,
                ..DriveConfig::new(dir.join("cache"))
            },
            listing.clone(),
            source.clone(),
        )
        .await
        .expect("drive");
        let fs = DriveFs::new(
            drive,
            tokio::runtime::Handle::current(),
            Now::new(|| UNIX_EPOCH),
        );
        (listing, source, fs)
    }

    #[tokio::test]
    async fn the_tree_reads_by_shown_names_with_stable_inodes_and_the_drives_sizes() {
        let dir = test_kit::Scratch::new("drive-fuse");
        let (listing, source, fs) = fs(&dir).await;
        listing.folder("f", None, "Work");
        listing.file("a", Some("f"), "a/b.txt", 10, "1");
        listing.online(
            "d",
            Some("f"),
            "Plan",
            "https://docs.example.test/d",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        );
        source.put("a", b"0123456789".to_vec());
        let work = fs.lookup(ROOT_INO, OsStr::new("Work")).expect("work");
        assert!(work.folder);
        let names: Vec<String> = fs
            .readdir(work.ino)
            .expect("readdir")
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, [".", "..", "a／b.txt", "Plan.gdoc"]);
        let file = fs.lookup(work.ino, OsStr::new("a／b.txt")).expect("file");
        assert_eq!((file.size, file.mtime), (10, 1_700_000_000));
        assert_eq!(fs.getattr(file.ino).expect("attr"), file);
        assert_eq!(fs.readdir(file.ino), Err(Errno::ENOTDIR));
        assert_eq!(
            fs.lookup(work.ino, OsStr::new("missing")),
            Err(Errno::ENOENT)
        );

        assert_eq!(
            fs.open(file.ino, true, false).await,
            Err(Errno::EROFS),
            "read-only"
        );
        assert_eq!(
            fs.create(work.ino, OsStr::new("new.txt"), REGULAR | 0o644),
            Err(Errno::EROFS)
        );
        assert_eq!(fs.file_attr(&file).perm, 0o444);
        let fh = fs.open(file.ino, false, false).await.expect("open");
        assert_eq!(fs.read(fh, 3, 4).await.expect("read"), b"3456");
        fs.release(fh);

        let doc = fs.lookup(work.ino, OsStr::new("Plan.gdoc")).expect("doc");
        let fh = fs.open(doc.ino, false, false).await.expect("open");
        let link = fs.read(fh, 0, 4096).await.expect("read");
        assert_eq!(link.len() as u64, doc.size);
        fs.release(fh);
    }

    #[tokio::test]
    async fn offline_a_read_is_enetdown_and_the_item_says_why() {
        let dir = test_kit::Scratch::new("drive-fuse");
        let (listing, source, fs) = fs(&dir).await;
        listing.file("a", None, "a.txt", 10, "1");
        source.set_offline(true);
        let file = fs.lookup(ROOT_INO, OsStr::new("a.txt")).expect("file");
        let fh = fs.open(file.ino, false, false).await.expect("open");
        assert_eq!(fs.read(fh, 0, 10).await, Err(Errno::ENETDOWN));
        let status = fs
            .drive
            .status(&NodeId::Remote(porter_drive::RemoteId("a".into())));
        assert_eq!(status.and_then(|s| s.reason), Some(Reason::Offline));
        assert_eq!(fs.read(999, 0, 1).await, Err(Errno::EBADF));
    }

    struct Writable {
        listing: Arc<MemoryListing>,
        source: Arc<MemorySource>,
        sink: Arc<MemorySink>,
        fs: DriveFs,
    }

    async fn writable(dir: &test_kit::Scratch) -> Writable {
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        let sink = Arc::new(MemorySink::new(listing.clone(), source.clone(), 4));
        let drive = Drive::open_writable(
            DriveConfig {
                chunk: 4,
                quiet: Duration::from_millis(20),
                ..DriveConfig::new(dir.join("cache"))
            },
            listing.clone(),
            source.clone(),
            sink.clone(),
        )
        .await
        .expect("drive");
        let fs = DriveFs::new(
            drive,
            tokio::runtime::Handle::current(),
            Now::new(|| UNIX_EPOCH),
        );
        Writable {
            listing,
            source,
            sink,
            fs,
        }
    }

    async fn sent(sink: &MemorySink, n: usize) -> Vec<Took> {
        let deadline = test_kit::Deadline::generous();
        while sink.took().len() < n {
            if deadline.passed() {
                deadline.fail(&format!("{n} changes sent"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        sink.took()
    }

    #[tokio::test]
    async fn a_file_made_written_and_closed_is_sent_through_the_core() {
        let dir = test_kit::Scratch::new("drive-fuse-write");
        let w = writable(&dir).await;
        let fs = &w.fs;
        let (attr, fh) = fs
            .create(ROOT_INO, OsStr::new("notes.txt"), REGULAR | 0o644)
            .expect("create");
        assert_eq!(fs.file_attr(&attr).perm, 0o644);
        assert_eq!(fs.write(fh, 0, b"hello ").await.expect("write"), 6);
        assert_eq!(fs.write(fh, 6, b"world").await.expect("write"), 5);
        fs.fsync(fh).expect("fsync");
        assert_eq!(fs.getattr(attr.ino).expect("attr").size, 11);
        assert_eq!(fs.read(fh, 0, 64).await.expect("read"), b"hello world");
        fs.release(fh);
        assert_eq!(
            sent(&w.sink, 1).await,
            [Took::New(None, "notes.txt".into(), 11)]
        );
        let again = fs
            .lookup(ROOT_INO, OsStr::new("notes.txt"))
            .expect("lookup");
        assert_eq!(again.ino, attr.ino, "the same inode once the drive has it");
    }

    #[tokio::test]
    async fn what_a_drive_cannot_hold_is_refused_with_its_errno() {
        let dir = test_kit::Scratch::new("drive-fuse-refuse");
        let w = writable(&dir).await;
        let fs = &w.fs;
        w.sink.names(|name| !name.contains(':'));
        w.listing.online(
            "d",
            None,
            "Plan",
            "https://docs.example.test/d",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        );
        assert_eq!(
            fs.create(ROOT_INO, OsStr::new("a:b"), REGULAR),
            Err(Errno::EINVAL)
        );
        assert_eq!(fs.mkdir(ROOT_INO, OsStr::new("c:d")), Err(Errno::EINVAL));
        // A fifo.
        assert_eq!(
            fs.mknod(ROOT_INO, OsStr::new("pipe"), 0o010_644),
            Err(Errno::EPERM)
        );
        let doc = fs.lookup(ROOT_INO, OsStr::new("Plan.gdoc")).expect("doc");
        assert_eq!(fs.open(doc.ino, true, false).await, Err(Errno::EPERM));
        assert_eq!(fs.file_attr(&doc).perm, 0o444);
        let made = fs
            .mknod(ROOT_INO, OsStr::new("empty"), REGULAR)
            .expect("mknod");
        assert_eq!(made.size, 0);
        assert_eq!(
            fs.create(ROOT_INO, OsStr::new("empty"), REGULAR),
            Err(Errno::EEXIST)
        );
        // A mode or an owner is taken, and ignored.
        assert_eq!(
            fs.setattr(made.ino, None, None, None).await,
            Ok(made.clone())
        );
    }

    #[tokio::test]
    async fn saving_by_rename_truncating_and_removing_go_through_the_core() {
        let dir = test_kit::Scratch::new("drive-fuse-save");
        let w = writable(&dir).await;
        let fs = &w.fs;
        w.listing.folder("f", None, "Work");
        w.listing.file("r", Some("f"), "report.txt", 3, "v1");
        w.source.put("r", b"old".to_vec());
        w.listing.file("x", Some("f"), "x.txt", 1, "v1");
        w.source.put("x", b"x".to_vec());
        let work = fs.lookup(ROOT_INO, OsStr::new("Work")).expect("work");
        // Held until all is done, so what is sent does not hang on the timing.
        fs.drive.set_paused(true);
        let (_, fh) = fs
            .create(work.ino, OsStr::new(".report.txt.swx~"), REGULAR)
            .expect("temp");
        fs.write(fh, 0, b"new text").await.expect("write");
        fs.release(fh);
        assert_eq!(
            fs.rename(
                work.ino,
                OsStr::new(".report.txt.swx~"),
                work.ino,
                OsStr::new("x.txt"),
                false
            )
            .await,
            Err(Errno::EEXIST),
            "not over a file unless asked"
        );
        fs.rename(
            work.ino,
            OsStr::new(".report.txt.swx~"),
            work.ino,
            OsStr::new("report.txt"),
            true,
        )
        .await
        .expect("save");
        let report = fs
            .lookup(work.ino, OsStr::new("report.txt"))
            .expect("report");
        let truncated = fs
            .setattr(report.ino, None, Some(3), None)
            .await
            .expect("truncate");
        assert_eq!(truncated.size, 3);
        fs.unlink(work.ino, OsStr::new("x.txt")).expect("unlink");
        assert_eq!(
            fs.rmdir(ROOT_INO, OsStr::new("Work")),
            Err(Errno::ENOTEMPTY)
        );
        fs.drive.set_paused(false);
        let took = sent(&w.sink, 2).await;
        assert!(took.contains(&Took::Replace("r".into(), 3)), "{took:?}");
        assert!(took.contains(&Took::Trash("x".into())), "{took:?}");
        assert_eq!(
            w.source.fetched(),
            [],
            "nothing was downloaded to replace it"
        );
    }
}
