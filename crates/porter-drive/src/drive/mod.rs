//! One drive's files on demand: its tree as shown, its cache and what each item is doing.
//!
//! The cache directory holds `index.json` (see `index`), `content/<key>` (one file per item,
//! `<key>` from its id and version, sparse while a fetch fills it), `local/<n>` (the bytes
//! written here and not sent, with `local/<n>.sending` beside one being sent: the copy taken
//! when its upload began) and `lock`, which one open drive holds at a time. Every call that
//! only looks is synchronous; reading bytes that are not here yet waits for them.
//!
//! What is changed here is shown at once and sent after (see `send`): the tree shown is the
//! listing with the changes not on the drive yet laid over it (see `view`).
//!
//! - **One drive per directory.** [`Drive::open`] takes an exclusive lock on `lock` (`flock`),
//!   waiting (async, yielding to the runtime) [`DriveConfig::lock_wait`] for a drive still open
//!   there to go, else
//!   [`DriveError::InUse`]. The lock goes with the drive, after its index is written.
//! - **What the index claims is on the disk.** A content file a fetch wrote to is synced
//!   (`fdatasync`) before an index that notes its ranges is written. An index read from the
//!   copy before the last write claims no ranges (it cannot vouch for files written since), and
//!   at open a row whose file is missing or of another size is dropped, as is a content file no
//!   row names.
//! - **A whole file is checked.** When every byte of a version is here, it is hashed and
//!   compared with the hash the listing has for it (MD5 for Google Drive, QuickXorHash for
//!   OneDrive, SHA-256 where a drive reports that); only then is it downloaded. With no hash
//!   (or a kind this crate does not compute) the listing's version is read again instead, which
//!   finds a change only once the listing has it. A mismatch drops the bytes: the item is stale,
//!   and is fetched again from the start when next asked.

mod actions;
mod conflict;
mod copy;
mod fetch;
mod send;
mod state;
#[cfg(test)]
mod tests;
mod view;
mod write;
#[cfg(test)]
mod write_tests;

pub use fetch::ReadError;
pub use state::{ItemState, Status};
pub use write::{WriteError, WriteHandle};

use crate::Reason;
use crate::index::{Dirty, Edit, Index, IndexFile};
use crate::limit::CacheLimit;
use crate::listing::{Listing, ListingError};
use crate::sink::Sink;
use crate::source::Source;
use fetch::Transfer;
use porter_ids::UnixSeconds;
use porter_sync::{ItemKind, RemoteId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, Semaphore, broadcast, watch};

/// The inode number of the top of the drive.
pub const ROOT_INO: u64 = 1;

/// How deep a path is walked before it is given up as a loop.
const DEPTH: usize = 64;

/// How long the index waits after a change before it is written, so a fetch that moves many
/// times a second writes it about once a second.
const FLUSH_AFTER: Duration = Duration::from_secs(1);

/// The lock file's name in the cache directory.
const LOCK: &str = "lock";

/// How often an open waiting for the lock tries it again.
const LOCK_EVERY: Duration = Duration::from_millis(20);

/// The quiet time [`DriveConfig::new`] gives: a file is sent once it has been closed and left
/// alone this long, not on each write.
pub const QUIET: Duration = Duration::from_secs(2);

/// What a node of the drive's tree is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeId {
    /// The top of the drive.
    Top,
    /// One of the drive's own items.
    Remote(RemoteId),
    /// A file or folder made on this computer and not on the drive yet, by its number. Once the
    /// drive has it, the number names its drive id ([`Drive::canonical`]).
    Local(u64),
}

/// What a node is, as a file system shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// A folder.
    Folder,
    /// A file with bytes.
    File,
    /// An online document's link file.
    Online {
        /// Where it opens.
        url: String,
        /// The MIME type a copy of it is saved as, when it has one.
        export: Option<String>,
    },
}

impl NodeKind {
    fn of(kind: &ItemKind) -> Self {
        match kind {
            ItemKind::Folder => NodeKind::Folder,
            ItemKind::Online { url, export } => NodeKind::Online {
                url: url.as_str().to_owned(),
                export: export.clone(),
            },
            _ => NodeKind::File,
        }
    }
}

/// A node with the name it is shown by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Which node.
    pub id: NodeId,
    /// The folder it is in (the top's is the top).
    pub parent: NodeId,
    /// The name it is shown by in that folder.
    pub name: String,
    /// What it is.
    pub kind: NodeKind,
    /// Its size as a file system shows it: the drive's size, or an online document's link file's.
    pub size: u64,
    /// When it last changed, when the drive says.
    pub modified: Option<UnixSeconds>,
}

/// What a node is and how large, without its name (which needs its folder read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attrs {
    /// What it is.
    pub kind: NodeKind,
    /// Its size.
    pub size: u64,
    /// When it last changed.
    pub modified: Option<UnixSeconds>,
}

/// The nodes whose details changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed(pub Vec<NodeId>);

/// Why a drive could not be opened or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DriveError {
    /// The listing could not be read.
    #[error(transparent)]
    Listing(#[from] ListingError),
    /// The cache directory could not be made or written.
    #[error("the cache could not be used: {0}")]
    Cache(#[from] std::io::Error),
    /// The drive was opened outside an async runtime, which its fetches run on.
    #[error("the drive needs an async runtime to fetch in")]
    NoRuntime,
    /// Another drive is still open over the same cache directory; try again once it is gone.
    #[error("the cache is in use by a drive still open over it")]
    InUse,
}

/// How a drive's cache is set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveConfig {
    /// The cache directory (`$XDG_CACHE_HOME/porter/drives/<account>/`).
    pub cache: PathBuf,
    /// How many bytes one fetch asks for.
    pub chunk: u64,
    /// The limit eviction keeps the cache under.
    pub limit: CacheLimit,
    /// How many downloads nobody is waiting on (Keep Downloaded, Download Now) run at once.
    pub parallel: usize,
    /// How long one fetch of a chunk may take before it is given up (its readers get
    /// [`ReadError::Stalled`] and the file downloads when next asked).
    pub deadline: Duration,
    /// How long [`Drive::open`] waits for another drive over the same directory to go.
    pub lock_wait: Duration,
    /// How long a file changed here is left closed and alone before it is sent.
    pub quiet: Duration,
    /// How the free space keeping everything downloaded is held against is found (measured each
    /// time it is asked, by default).
    pub free_space: FreeSpace,
}

impl DriveConfig {
    /// A cache in `dir`, with chunks of a mebibyte, the automatic limit, four downloads at once,
    /// two minutes for a chunk, two seconds for the lock and [`QUIET`] before a change is sent;
    /// the free space is measured.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            cache: dir,
            chunk: 1 << 20,
            limit: CacheLimit::Automatic,
            parallel: 4,
            deadline: Duration::from_secs(120),
            lock_wait: Duration::from_secs(2),
            quiet: QUIET,
            free_space: FreeSpace::Measured,
        }
    }
}

/// How the free space of a drive's cache is found: what keeping the whole drive downloaded is
/// held against before anything is fetched for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FreeSpace {
    /// Measured on the cache's file system each time it is asked; when the file system cannot be
    /// asked it is not known.
    #[default]
    Measured,
    /// This many bytes (a test's figure, or a host's share of the disk).
    Bytes(u64),
    /// Not known. An unknown free space is never short: downloads go ahead, and a disk that fills
    /// shows on the item as any download that fails does.
    Unknown,
}

/// Why an item's last fetch failed, as its status shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// Why, for whoever shows it.
    pub reason: Reason,
    /// It may pass by itself (no network, a stall): cleared when the drive is reached again.
    pub transient: bool,
}

/// A file open for writing: how many have it open, and when it was last written or closed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Writing {
    handles: usize,
    last: Instant,
}

pub(crate) struct Inner {
    listing: Arc<dyn Listing>,
    source: Arc<dyn Source>,
    /// Where changes go; `None`: the drive is shown read-only.
    sink: Option<Arc<dyn Sink>>,
    dir: PathBuf,
    chunk: u64,
    file: IndexFile,
    index: Mutex<Index>,
    dirty: AtomicBool,
    /// Content files written to since the index was last written: synced before it is.
    unsynced: Mutex<HashSet<PathBuf>>,
    #[cfg(test)]
    synced: Mutex<Vec<PathBuf>>,
    flush: Arc<Notify>,
    transfers: Mutex<HashMap<RemoteId, Arc<Transfer>>>,
    open: Mutex<HashMap<NodeId, usize>>,
    errors: Mutex<HashMap<RemoteId, Failure>>,
    limit: Mutex<CacheLimit>,
    /// "Keep Everything Downloaded": every item counts as kept.
    keep_all: AtomicBool,
    /// Keeping everything downloaded needs more room than the cache's file system has free; its
    /// watchers hear each change, and only a change.
    short_of_room: watch::Sender<bool>,
    /// How the free space is found.
    free_space: FreeSpace,
    inodes: Mutex<HashMap<u64, NodeId>>,
    offline: AtomicBool,
    quota: Mutex<Option<(u64, Option<u64>)>>,
    events: broadcast::Sender<Changed>,
    background: Arc<Semaphore>,
    runtime: tokio::runtime::Handle,
    deadline: Duration,
    /// The files open for writing, and when each was last touched.
    writing: Mutex<HashMap<NodeId, Writing>>,
    quiet: Duration,
    /// The change being sent now.
    sending: Mutex<Option<send::Sent>>,
    /// Wakes the sender: something changed, or it may send again.
    wake: Arc<Notify>,
    /// Told when a listing pass is wanted (a change was sent, or the drive's version is newer).
    wanted: Arc<Notify>,
    paused: AtomicBool,
    /// The drive refused the account's sign-in: nothing is sent until it is signed in again.
    signed_out: AtomicBool,
    /// Changes the drive could not take now, with the reason (in use there), until their
    /// next try.
    waiting: Mutex<HashMap<NodeId, Reason>>,
    /// Changes the drive answered with a newer version: held until a listing pass shows it.
    stale: Mutex<HashSet<NodeId>>,
    /// When the sender tries again after the drive could not take a change, and how long it
    /// waited last.
    backoff: Mutex<Option<(Instant, Duration)>>,
    /// One lock per file of bytes written here (`local/<n>`), taken around each change of the
    /// bytes and of what the index says of them, by a write, a snapshot for an upload and the
    /// upload's landing. Always taken before the index's lock.
    files: Mutex<HashMap<u64, Arc<Mutex<()>>>>,
    /// The files open for writing, by handle: what each writes to now (`None`: the file it
    /// opened was saved over, and it writes nowhere).
    handles: Mutex<HashMap<u64, Option<NodeId>>>,
    next_handle: AtomicU64,
    /// Upload sessions whose bytes changed: the sender gives each up before its next step.
    abandoned: Mutex<Vec<porter_sync::Session>>,
    /// A file was made in `local/` since the folder was last synced.
    local_made: AtomicBool,
    /// Held by each write of the index, from taking its copy to saving it.
    flushing: Mutex<()>,
    /// A test's hold inside a write, between its bytes and what the index says of them: met
    /// twice there.
    #[cfg(test)]
    write_hook: Mutex<Option<Arc<std::sync::Barrier>>>,
    /// A test's holds at named places, each met twice there, as `write_hook` is.
    #[cfg(test)]
    hooks: Mutex<HashMap<&'static str, Arc<std::sync::Barrier>>>,
    /// A test's failures at named places: the error a step there meets instead of the disk.
    #[cfg(test)]
    faults: Mutex<HashMap<&'static str, std::io::ErrorKind>>,
    /// The cache directory's lock, held for as long as the drive lives. A field drops after
    /// `drop` has written the index, so the next drive reads it whole.
    _lock: std::fs::File,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Err(why) = self.flush_now() {
            eprintln!("porter-drive: the cache's index could not be written: {why}");
        }
        self.flush.notify_one();
        self.wake.notify_one();
    }
}

/// Takes the lock of the cache directory `dir`, waiting up to `wait` for the drive holding it.
/// The wait yields to the runtime, so the task that lets the previous drive go can run.
async fn lock(dir: &Path, wait: Duration) -> Result<std::fs::File, DriveError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(LOCK_EVERY).await;
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(DriveError::InUse),
            Err(std::fs::TryLockError::Error(why)) => return Err(why.into()),
        }
    }
}

/// Makes `index` agree with the cache directory `dir`: a row whose content file is missing or
/// of another size is dropped (it claims what is not there), a content file no row names is
/// removed, a change's bytes are as long as its file is (they were written before a crash the
/// index did not see), and a file of bytes the index does not know (written before a crash) is
/// taken in as a file made here, so nothing written here is lost and its number is never given
/// out again.
fn reconcile(dir: &Path, index: &mut Index) -> std::io::Result<bool> {
    let mut changed = false;
    index.items.retain(|id, row| {
        let path = dir
            .join("content")
            .join(key(&RemoteId(id.clone()), &row.version));
        let fits = std::fs::metadata(&path).is_ok_and(|meta| meta.len() == row.size);
        changed |= !fits;
        fits
    });
    let named: HashSet<String> = index
        .items
        .iter()
        .map(|(id, row)| key(&RemoteId(id.clone()), &row.version))
        .collect();
    for file in std::fs::read_dir(dir.join("content"))? {
        let file = file?;
        if !file
            .file_name()
            .to_str()
            .is_some_and(|name| named.contains(name))
        {
            let _ = std::fs::remove_file(file.path());
        }
    }
    let mut known: HashMap<u64, NodeId> = HashMap::new();
    for edit in index.edits.values() {
        if let Some(dirty) = &edit.dirty {
            known.insert(dirty.file, edit.id.clone());
        }
    }
    for file in std::fs::read_dir(dir.join("local"))? {
        let file = file?;
        let name = file.file_name();
        let name = name.to_str().unwrap_or_default();
        if name.ends_with(".sending") {
            // A copy taken for an upload that did not land: taken again when it is sent.
            let _ = std::fs::remove_file(file.path());
            continue;
        }
        let Ok(n) = name.parse::<u64>() else {
            continue;
        };
        index.next_local = index.next_local.max(n + 1);
        let meta = file.metadata()?;
        if let Some(id) = known.get(&n) {
            if let Some(dirty) = index.edits.get_mut(id).and_then(|e| e.dirty.as_mut())
                && dirty.size != meta.len()
            {
                dirty.size = meta.len();
                changed = true;
            }
            continue;
        }
        let made = meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let dirty = Dirty {
            file: n,
            size: meta.len(),
            modified: made,
            generation: index.take_generation(),
        };
        let name = format!("{} {n}", copy::RECOVERED);
        let mut edit = Edit::made_here(n, NodeId::Top, name, Some(dirty), made);
        edit.seq = index.take_seq();
        index.edits.insert(edit.id.clone(), edit);
        changed = true;
    }
    // A change whose bytes went (a crash before they were written) keeps what else it was.
    for edit in index.edits.values_mut() {
        if let Some(dirty) = &edit.dirty
            && !dir.join("local").join(dirty.file.to_string()).exists()
        {
            edit.dirty = None;
            changed = true;
        }
    }
    // What was made here and removed while it was open, and what lost its bytes, is gone; no
    // one has anything open after a restart.
    let lost: Vec<NodeId> = index
        .edits
        .values()
        .filter(|edit| {
            edit.base.is_none() && (edit.removed || (!edit.folder && edit.dirty.is_none()))
        })
        .map(|edit| edit.id.clone())
        .collect();
    for id in lost {
        if let Some(dirty) = index.edits.remove(&id).and_then(|edit| edit.dirty) {
            let _ = std::fs::remove_file(dir.join("local").join(dirty.file.to_string()));
        }
        changed = true;
    }
    let before = (index.aliases.len(), index.inos.len());
    index.prune();
    changed |= before != (index.aliases.len(), index.inos.len());
    Ok(changed)
}

/// Seconds since the epoch, now.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every critical section is a plain update of a map.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One drive's files on demand. Clones share the drive.
#[derive(Clone)]
pub struct Drive {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Drive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Drive")
            .field("cache", &self.inner.dir)
            .finish_non_exhaustive()
    }
}

/// The content file name of `id` at `version`.
fn key(id: &RemoteId, version: &str) -> String {
    let digest = Sha256::digest(format!("{}\0{version}", id.0).as_bytes());
    hex::encode(&digest[..20])
}

/// The text an inode number is made from, and the index names an item by when it keeps the
/// inode number of what it was.
pub(crate) fn key_text(id: &NodeId) -> String {
    match id {
        NodeId::Top => String::new(),
        NodeId::Remote(id) => format!("r:{}", id.0),
        NodeId::Local(n) => format!("l:{n}"),
    }
}

/// The inode number of `id`: stable for a remote id, the same after a restart.
fn ino_for(id: &NodeId) -> u64 {
    if *id == NodeId::Top {
        return ROOT_INO;
    }
    let digest = Sha256::digest(key_text(id).as_bytes());
    let ino = u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]));
    // 0 is no inode and 1 the top.
    ino.max(2)
}

impl Drive {
    /// The drive over `listing`, fetching from `source`, with its cache as `config` says, shown
    /// read-only. Must be called inside a Tokio runtime: its fetches and the index's writes run
    /// there. If another drive holds the cache, it waits (without blocking the runtime) up to
    /// [`DriveConfig::lock_wait`] for that drive to go.
    ///
    /// # Errors
    /// No runtime, the cache directory could not be made, or it stayed in use.
    pub async fn open(
        config: DriveConfig,
        listing: Arc<dyn Listing>,
        source: Arc<dyn Source>,
    ) -> Result<Drive, DriveError> {
        Self::open_with(config, listing, source, None).await
    }

    /// The drive as [`Drive::open`] opens it, and changed here: each change is kept in the cache
    /// first, then sent to `sink`.
    ///
    /// # Errors
    /// No runtime, the cache directory could not be made, or it stayed in use.
    pub async fn open_writable(
        config: DriveConfig,
        listing: Arc<dyn Listing>,
        source: Arc<dyn Source>,
        sink: Arc<dyn Sink>,
    ) -> Result<Drive, DriveError> {
        Self::open_with(config, listing, source, Some(sink)).await
    }

    async fn open_with(
        config: DriveConfig,
        listing: Arc<dyn Listing>,
        source: Arc<dyn Source>,
        sink: Option<Arc<dyn Sink>>,
    ) -> Result<Drive, DriveError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| DriveError::NoRuntime)?;
        std::fs::create_dir_all(config.cache.join("content"))?;
        std::fs::create_dir_all(config.cache.join("local"))?;
        let lock = lock(&config.cache, config.lock_wait).await?;
        let file = IndexFile::new(&config.cache);
        let mut index = file.load();
        let changed = reconcile(&config.cache, &mut index)?;
        let (events, _) = broadcast::channel(1024);
        let mut inodes = HashMap::from([(ROOT_INO, NodeId::Top)]);
        for (n, id) in &index.aliases {
            inodes.insert(ino_for(&NodeId::Local(*n)), id.clone());
        }
        let inner = Arc::new(Inner {
            listing,
            source,
            sink,
            dir: config.cache,
            chunk: config.chunk.max(1),
            file,
            index: Mutex::new(index),
            dirty: AtomicBool::new(changed),
            unsynced: Mutex::default(),
            #[cfg(test)]
            synced: Mutex::default(),
            flush: Arc::new(Notify::new()),
            transfers: Mutex::default(),
            open: Mutex::default(),
            errors: Mutex::default(),
            limit: Mutex::new(config.limit),
            keep_all: AtomicBool::new(false),
            short_of_room: watch::channel(false).0,
            free_space: config.free_space,
            inodes: Mutex::new(inodes),
            offline: AtomicBool::new(false),
            quota: Mutex::new(None),
            events,
            background: Arc::new(Semaphore::new(config.parallel.max(1))),
            runtime,
            deadline: config.deadline,
            writing: Mutex::default(),
            quiet: config.quiet,
            sending: Mutex::new(None),
            wake: Arc::new(Notify::new()),
            wanted: Arc::new(Notify::new()),
            paused: AtomicBool::new(false),
            signed_out: AtomicBool::new(false),
            waiting: Mutex::default(),
            stale: Mutex::default(),
            backoff: Mutex::new(None),
            files: Mutex::default(),
            handles: Mutex::default(),
            next_handle: AtomicU64::new(1),
            abandoned: Mutex::default(),
            local_made: AtomicBool::new(false),
            flushing: Mutex::new(()),
            #[cfg(test)]
            write_hook: Mutex::default(),
            #[cfg(test)]
            hooks: Mutex::default(),
            #[cfg(test)]
            faults: Mutex::default(),
            _lock: lock,
        });
        let weak = Arc::downgrade(&inner);
        inner.runtime.spawn(flusher(weak, Arc::clone(&inner.flush)));
        if inner.sink.is_some() {
            let weak = Arc::downgrade(&inner);
            inner
                .runtime
                .spawn(send::sender(weak, Arc::clone(&inner.wake)));
        }
        if changed {
            inner.flush_now()?;
        }
        Ok(Drive { inner })
    }

    /// The cache directory.
    pub fn cache_dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Whether what is changed here is sent to the drive ([`Drive::open_writable`]); a drive
    /// opened read-only refuses every change.
    pub fn writable(&self) -> bool {
        self.inner.sink.is_some()
    }

    /// Hears which nodes' details change from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Changed> {
        self.inner.events.subscribe()
    }

    /// Writes the index now if it changed.
    ///
    /// # Errors
    /// It could not be written.
    pub fn flush(&self) -> std::io::Result<()> {
        self.inner.flush_now()
    }

    /// The limit eviction keeps the cache under.
    pub fn limit(&self) -> CacheLimit {
        *held(&self.inner.limit)
    }

    /// Sets the limit and evicts what is past it at once.
    pub fn set_limit(&self, limit: CacheLimit) {
        *held(&self.inner.limit) = limit;
        self.inner.evict();
    }

    /// The account's quota as the drive last said: bytes used and, when limited, the total.
    pub fn quota(&self) -> Option<(u64, Option<u64>)> {
        *held(&self.inner.quota)
    }

    /// Notes the account's quota.
    pub fn set_quota(&self, used: u64, total: Option<u64>) {
        *held(&self.inner.quota) = Some((used, total));
    }

    /// Whether the drive could not be reached the last time it was asked.
    pub fn offline(&self) -> bool {
        self.inner.offline.load(Ordering::SeqCst)
    }

    /// Notes whether the drive can be reached (the listing says so too). Reached again, what
    /// failed for want of it says so no more: it downloads when next asked, and what waits to
    /// be sent is sent.
    pub fn set_offline(&self, offline: bool) {
        if self.inner.offline.swap(offline, Ordering::SeqCst) && !offline {
            self.inner.came_online();
            *held(&self.inner.backoff) = None;
            self.inner.wake.notify_one();
        }
    }

    /// Every item whose state is not plain "in the cloud": cached, downloading or failed. What a
    /// listener that missed changes asks again about.
    pub fn tracked(&self) -> Vec<NodeId> {
        let index = held(&self.inner.index);
        let mut ids: std::collections::BTreeSet<NodeId> = index
            .items
            .keys()
            .map(|id| NodeId::Remote(RemoteId(id.clone())))
            .collect();
        ids.extend(index.edits.keys().cloned());
        drop(index);
        ids.extend(
            held(&self.inner.transfers)
                .keys()
                .cloned()
                .map(NodeId::Remote),
        );
        ids.extend(held(&self.inner.errors).keys().cloned().map(NodeId::Remote));
        ids.into_iter().collect()
    }

    /// The inode number of `id`. An item made here keeps its number once the drive has it.
    pub fn ino(&self, id: &NodeId) -> u64 {
        self.inner.ino(id)
    }

    /// The node an inode number this drive gave out names.
    pub fn by_ino(&self, ino: u64) -> Option<NodeId> {
        let id = held(&self.inner.inodes).get(&ino).cloned()?;
        Some(self.inner.canonical(&id))
    }

    /// The node `id` names now: an item made here that the drive has since given an id (or
    /// that was saved over another file) is named by that.
    pub fn canonical(&self, id: &NodeId) -> NodeId {
        self.inner.canonical(id)
    }

    /// Whether `id` stays on this computer and is never sent (see [`crate::local_only`]): a
    /// list of the drive's items leaves it out.
    pub fn local_only(&self, id: &NodeId) -> bool {
        self.inner.local_only(id)
    }

    /// The nodes directly inside `folder`, each with its shown name; none for a file.
    ///
    /// # Errors
    /// The listing could not be read.
    pub fn children(&self, folder: &NodeId) -> Result<Vec<Node>, DriveError> {
        self.inner.children(folder)
    }

    /// The node `name` in `folder`.
    ///
    /// # Errors
    /// The listing could not be read.
    pub fn lookup(&self, folder: &NodeId, name: &str) -> Result<Option<Node>, DriveError> {
        Ok(self
            .inner
            .children(folder)?
            .into_iter()
            .find(|node| node.name == name))
    }

    /// The node `id`, with its shown name.
    ///
    /// # Errors
    /// The listing could not be read.
    pub fn node(&self, id: &NodeId) -> Result<Option<Node>, DriveError> {
        self.inner.node(id)
    }

    /// What `id` is and how large.
    ///
    /// # Errors
    /// The listing could not be read.
    pub fn attrs(&self, id: &NodeId) -> Result<Option<Attrs>, DriveError> {
        self.inner.attrs(id)
    }

    /// The node at `path` below the top of the drive (`""` is the top), by shown names.
    ///
    /// # Errors
    /// The listing could not be read.
    pub fn resolve(&self, path: &Path) -> Result<Option<Node>, DriveError> {
        let mut at = self.inner.top();
        for part in path.components() {
            let Component::Normal(name) = part else {
                continue;
            };
            let Some(name) = name.to_str() else {
                return Ok(None);
            };
            match self.lookup(&at.id, name)? {
                Some(node) => at = node,
                None => return Ok(None),
            }
        }
        Ok(Some(at))
    }

    /// Where `id` is below the top of the drive, by shown names; `None` when it is not there.
    ///
    /// # Errors
    /// The listing could not be read.
    pub fn path_of(&self, id: &NodeId) -> Result<Option<PathBuf>, DriveError> {
        let mut names = Vec::new();
        let mut at = id.clone();
        for _ in 0..DEPTH {
            if at == NodeId::Top {
                names.reverse();
                return Ok(Some(names.iter().collect()));
            }
            let Some(node) = self.inner.node(&at)? else {
                return Ok(None);
            };
            names.push(node.name);
            at = node.parent;
        }
        Ok(None)
    }
}

impl Inner {
    fn emit(&self, ids: Vec<NodeId>) {
        if !ids.is_empty() {
            let _ = self.events.send(Changed(ids));
        }
    }

    /// Notes that the index changed: the flusher writes it soon.
    fn touched(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        self.flush.notify_one();
    }

    /// Notes that `file`'s data changed: it is synced before the index is next written.
    fn wrote(&self, file: &Path) {
        held(&self.unsynced).insert(file.to_path_buf());
    }

    /// Writes the index now if it changed, after syncing every content file written to since
    /// the last write: the index never claims bytes that are not on the disk.
    fn flush_now(&self) -> std::io::Result<()> {
        // One at a time: an older copy of the index is never written over a newer one.
        let _one = held(&self.flushing);
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        // The index first, then the files: every range it notes was written, and its file
        // named in `unsynced`, before it was noted.
        let index = held(&self.index).clone();
        #[cfg(test)]
        self.at_hook("flush");
        if self.local_made.swap(false, Ordering::SeqCst) {
            // A file made in `local/` is in the folder before an index that names it is
            // written.
            let local = self.dir.join("local");
            if let Err(why) = std::fs::File::open(&local).and_then(|dir| dir.sync_all()) {
                self.local_made.store(true, Ordering::SeqCst);
                self.dirty.store(true, Ordering::SeqCst);
                return Err(why);
            }
            #[cfg(test)]
            held(&self.synced).push(local);
        }
        let files: Vec<PathBuf> = held(&self.unsynced).drain().collect();
        let failed = |files: &[PathBuf], why: std::io::Error| {
            held(&self.unsynced).extend(files.iter().cloned());
            self.dirty.store(true, Ordering::SeqCst);
            why
        };
        for (at, path) in files.iter().enumerate() {
            match std::fs::File::open(path).and_then(|file| file.sync_data()) {
                Ok(()) => {
                    #[cfg(test)]
                    held(&self.synced).push(path.clone());
                }
                // Removed with its row (a removal, a newer version): the index that drops the
                // row is written next, and an open drops a row whose file is missing.
                Err(why) if why.kind() == std::io::ErrorKind::NotFound => {
                    self.dirty.store(true, Ordering::SeqCst);
                }
                Err(why) => return Err(failed(&files[at..], why)),
            }
        }
        self.file.save(&index).map_err(|why| failed(&[], why))
    }

    fn content(&self, id: &RemoteId, version: &str) -> PathBuf {
        self.dir.join("content").join(key(id, version))
    }

    fn local_file(&self, n: u64) -> PathBuf {
        self.dir.join("local").join(n.to_string())
    }

    /// Whether `id` is kept, itself, by a folder above it, or with the whole drive, as `index`
    /// says. When the folders above it cannot be read it counts as kept: nothing is evicted on a
    /// guess.
    fn kept_in(&self, index: &Index, id: &RemoteId) -> bool {
        self.keeps_all()
            || index.kept.contains(&id.0)
            || self.listing.ancestors(id).map_or(true, |above| {
                above.iter().any(|folder| index.kept.contains(&folder.0))
            })
    }

    /// Whether a folder above `id` is kept, or the whole drive is; when the folders above it
    /// cannot be read, yes.
    fn kept_by_folder(&self, id: &NodeId) -> bool {
        if self.keeps_all() {
            return true;
        }
        let above = match self.canonical(id) {
            NodeId::Top => return false,
            NodeId::Remote(id)
                if !held(&self.index)
                    .edits
                    .contains_key(&NodeId::Remote(id.clone())) =>
            {
                self.listing.ancestors(&id)
            }
            other => {
                // Up through what was made or moved here to the first of the drive's folders.
                let mut at = other;
                let mut found = Ok(Vec::new());
                for _ in 0..DEPTH {
                    match self.parent_of(&at) {
                        Ok(Some(NodeId::Remote(parent))) => {
                            found = self.listing.ancestors(&parent).map(|mut above| {
                                above.insert(0, parent);
                                above
                            });
                            break;
                        }
                        Ok(Some(NodeId::Local(n))) => at = NodeId::Local(n),
                        _ => break,
                    }
                }
                found
            }
        };
        let index = held(&self.index);
        above.map_or(true, |above| {
            above.iter().any(|folder| index.kept.contains(&folder.0))
        })
    }

    /// Whether everything is kept downloaded.
    fn keeps_all(&self) -> bool {
        self.keep_all.load(Ordering::SeqCst)
    }

    fn is_open(&self, id: &NodeId) -> bool {
        held(&self.open).get(id).is_some_and(|n| *n > 0)
    }
}

#[cfg(test)]
impl Inner {
    /// Waits at the test's hold inside a write, if it set one.
    fn at_write_hook(&self) {
        let hook = held(&self.write_hook).clone();
        if let Some(hold) = hook {
            hold.wait();
            hold.wait();
        }
    }

    /// Waits at the test's hold `name`, if it set one.
    fn at_hook(&self, name: &str) {
        let hook = held(&self.hooks).get(name).cloned();
        if let Some(hold) = hook {
            hold.wait();
            hold.wait();
        }
    }

    /// The failure the test set at `name`, if any.
    fn fault(&self, name: &str) -> std::io::Result<()> {
        match held(&self.faults).get(name) {
            Some(kind) => Err(std::io::Error::from(*kind)),
            None => Ok(()),
        }
    }
}

/// Writes the index about once a second while it changes, for as long as the drive lives (its
/// drop wakes this to end it).
async fn flusher(weak: Weak<Inner>, flush: Arc<Notify>) {
    loop {
        flush.notified().await;
        if weak.strong_count() == 0 {
            return;
        }
        tokio::time::sleep(FLUSH_AFTER).await;
        let Some(inner) = weak.upgrade() else { return };
        // Syncing files waits on the disk: off the runtime's threads.
        let flushed = tokio::task::spawn_blocking(move || inner.flush_now()).await;
        if let Ok(Err(why)) = flushed {
            eprintln!("porter-drive: the cache's index could not be written: {why}");
        }
    }
}
