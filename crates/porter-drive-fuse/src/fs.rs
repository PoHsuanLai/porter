//! fuser's calls over a drive.

use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, LockOwner,
    OpenAccMode, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, Request,
};
use porter_drive::{Drive, NodeId, NodeKind, ROOT_INO, ReadError};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

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

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One drive as a read-only file system.
#[derive(Debug)]
pub struct DriveFs {
    drive: Drive,
    runtime: tokio::runtime::Handle,
    uid: u32,
    gid: u32,
    handles: Mutex<HashMap<u64, NodeId>>,
    next: AtomicU64,
}

impl DriveFs {
    /// The file system over `drive`, its waiting reads on `runtime`, its files owned by this
    /// process's user.
    pub fn new(drive: Drive, runtime: tokio::runtime::Handle) -> Self {
        Self {
            drive,
            runtime,
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
            handles: Mutex::default(),
            next: AtomicU64::new(1),
        }
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

    /// Opens `ino` to read: a file not here starts downloading. Opening to write is `EROFS`.
    ///
    /// # Errors
    /// `EROFS` to write, `EISDIR` for a folder, `ENOENT` when it is not there.
    pub fn open(&self, ino: u64, write: bool) -> Result<u64, Errno> {
        if write {
            return Err(Errno::EROFS);
        }
        let id = self.node_of(ino)?;
        self.drive.open_file(&id).map_err(errno_of)?;
        let fh = self.next.fetch_add(1, Ordering::Relaxed);
        held(&self.handles).insert(fh, id);
        Ok(fh)
    }

    /// `size` bytes of the file open as `fh` from `offset`, waiting for them when they are not
    /// here yet.
    ///
    /// # Errors
    /// `EBADF` for a handle not open, else as [`errno_of`] says.
    pub async fn read(&self, fh: u64, offset: u64, size: u32) -> Result<Vec<u8>, Errno> {
        let id = held(&self.handles).get(&fh).cloned().ok_or(Errno::EBADF)?;
        self.drive
            .read(&id, offset, u64::from(size))
            .await
            .map_err(errno_of)
    }

    /// Closes `fh`.
    pub fn release(&self, fh: u64) {
        if let Some(id) = held(&self.handles).remove(&fh) {
            self.drive.release(&id);
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
            perm: if attr.folder { 0o555 } else { 0o444 },
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
        match self.0.open(ino.0, write) {
            Ok(fh) => reply.opened(FileHandle(fh), FopenFlags::empty()),
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
    use porter_drive::testing::{MemoryListing, MemorySource};
    use porter_drive::{DriveConfig, words};
    use std::sync::Arc;

    fn fs(dir: &porter_fake::Scratch) -> (Arc<MemoryListing>, Arc<MemorySource>, DriveFs) {
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
        .expect("drive");
        let fs = DriveFs::new(drive, tokio::runtime::Handle::current());
        (listing, source, fs)
    }

    #[tokio::test]
    async fn the_tree_reads_by_shown_names_with_stable_inodes_and_the_drives_sizes() {
        let dir = porter_fake::Scratch::new("drive-fuse");
        let (listing, source, fs) = fs(&dir);
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

        assert_eq!(fs.open(file.ino, true), Err(Errno::EROFS), "read-only");
        let fh = fs.open(file.ino, false).expect("open");
        assert_eq!(fs.read(fh, 3, 4).await.expect("read"), b"3456");
        fs.release(fh);

        let doc = fs.lookup(work.ino, OsStr::new("Plan.gdoc")).expect("doc");
        let fh = fs.open(doc.ino, false).expect("open");
        let link = fs.read(fh, 0, 4096).await.expect("read");
        assert_eq!(link.len() as u64, doc.size);
        fs.release(fh);
    }

    #[tokio::test]
    async fn offline_a_read_is_enetdown_and_the_item_says_why() {
        let dir = porter_fake::Scratch::new("drive-fuse");
        let (listing, source, fs) = fs(&dir);
        listing.file("a", None, "a.txt", 10, "1");
        source.set_offline(true);
        let file = fs.lookup(ROOT_INO, OsStr::new("a.txt")).expect("file");
        let fh = fs.open(file.ino, false).expect("open");
        assert_eq!(fs.read(fh, 0, 10).await, Err(Errno::ENETDOWN));
        let status = fs
            .drive
            .status(&NodeId::Remote(porter_drive::RemoteId("a".into())));
        assert_eq!(
            status.and_then(|s| s.words).as_deref(),
            Some(words::OFFLINE)
        );
        assert_eq!(fs.read(999, 0, 1).await, Err(Errno::EBADF));
    }
}
