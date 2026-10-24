//! One drive's files on demand: its tree as shown, its cache and what each item is doing.
//!
//! The cache directory holds `index.json` (see `index`), `content/<key>` (one file per item,
//! `<key>` from its id and version, sparse while a fetch fills it) and `local/<n>` (the files
//! made here). Every call that only looks is synchronous; reading bytes that are not here yet
//! waits for them.

mod actions;
mod fetch;
mod state;
#[cfg(test)]
mod tests;

pub use fetch::ReadError;
pub use state::{ItemState, Status};

use crate::index::{Index, IndexFile, LocalRow};
use crate::limit::CacheLimit;
use crate::listing::{Entry, Listing, ListingError};
use crate::naming;
use crate::online;
use crate::source::Source;
use fetch::Transfer;
use porter_core::UnixSeconds;
use porter_sync::{ItemKind, RemoteId};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore, broadcast};

/// The inode number of the top of the drive.
pub const ROOT_INO: u64 = 1;

/// How deep a path is walked before it is given up as a loop.
const DEPTH: usize = 64;

/// How long the index waits after a change before it is written, so a fetch that moves many
/// times a second writes it about once a second.
const FLUSH_AFTER: Duration = Duration::from_secs(1);

/// What a node of the drive's tree is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    /// The top of the drive.
    Top,
    /// One of the drive's own items.
    Remote(RemoteId),
    /// A file made on this computer and not on the drive yet, by its number.
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
}

impl DriveConfig {
    /// A cache in `dir`, with chunks of a mebibyte, the automatic limit and four downloads at once.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            cache: dir,
            chunk: 1 << 20,
            limit: CacheLimit::Automatic,
            parallel: 4,
        }
    }
}

pub(crate) struct Inner {
    listing: Arc<dyn Listing>,
    source: Arc<dyn Source>,
    dir: PathBuf,
    chunk: u64,
    file: IndexFile,
    index: Mutex<Index>,
    dirty: AtomicBool,
    flush: Arc<Notify>,
    transfers: Mutex<HashMap<RemoteId, Arc<Transfer>>>,
    open: Mutex<HashMap<NodeId, usize>>,
    errors: Mutex<HashMap<RemoteId, String>>,
    limit: Mutex<CacheLimit>,
    inodes: Mutex<HashMap<u64, NodeId>>,
    offline: AtomicBool,
    quota: Mutex<Option<(u64, Option<u64>)>>,
    events: broadcast::Sender<Changed>,
    background: Arc<Semaphore>,
    runtime: tokio::runtime::Handle,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if self.dirty.swap(false, Ordering::SeqCst) {
            let _ = self.file.save(&held(&self.index));
        }
        self.flush.notify_one();
    }
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
    digest.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

/// The inode number of `id`: stable for a remote id, the same after a restart.
fn ino_for(id: &NodeId) -> u64 {
    let text = match id {
        NodeId::Top => return ROOT_INO,
        NodeId::Remote(id) => format!("r:{}", id.0),
        NodeId::Local(n) => format!("l:{n}"),
    };
    let digest = Sha256::digest(text.as_bytes());
    let ino = u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]));
    // 0 is no inode and 1 the top.
    ino.max(2)
}

impl Drive {
    /// The drive over `listing`, fetching from `source`, with its cache as `config` says. Must
    /// be called inside a Tokio runtime: its fetches and the index's writes run there.
    ///
    /// # Errors
    /// No runtime, or the cache directory could not be made.
    pub fn open(
        config: DriveConfig,
        listing: Arc<dyn Listing>,
        source: Arc<dyn Source>,
    ) -> Result<Drive, DriveError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| DriveError::NoRuntime)?;
        std::fs::create_dir_all(config.cache.join("content"))?;
        std::fs::create_dir_all(config.cache.join("local"))?;
        let file = IndexFile::new(&config.cache);
        let index = file.load();
        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new(Inner {
            listing,
            source,
            dir: config.cache,
            chunk: config.chunk.max(1),
            file,
            index: Mutex::new(index),
            dirty: AtomicBool::new(false),
            flush: Arc::new(Notify::new()),
            transfers: Mutex::default(),
            open: Mutex::default(),
            errors: Mutex::default(),
            limit: Mutex::new(config.limit),
            inodes: Mutex::new(HashMap::from([(ROOT_INO, NodeId::Top)])),
            offline: AtomicBool::new(false),
            quota: Mutex::new(None),
            events,
            background: Arc::new(Semaphore::new(config.parallel.max(1))),
            runtime,
        });
        let weak = Arc::downgrade(&inner);
        inner.runtime.spawn(flusher(weak, Arc::clone(&inner.flush)));
        Ok(Drive { inner })
    }

    /// The cache directory.
    pub fn cache_dir(&self) -> &Path {
        &self.inner.dir
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

    /// Notes whether the drive can be reached (the listing says so too).
    pub fn set_offline(&self, offline: bool) {
        self.inner.offline.store(offline, Ordering::SeqCst);
    }

    /// The inode number of `id`.
    pub fn ino(&self, id: &NodeId) -> u64 {
        self.inner.ino(id)
    }

    /// The node an inode number this drive gave out names.
    pub fn by_ino(&self, ino: u64) -> Option<NodeId> {
        held(&self.inner.inodes).get(&ino).cloned()
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

    fn flush_now(&self) -> std::io::Result<()> {
        if self.dirty.swap(false, Ordering::SeqCst) {
            let index = held(&self.index).clone();
            if let Err(why) = self.file.save(&index) {
                self.dirty.store(true, Ordering::SeqCst);
                return Err(why);
            }
        }
        Ok(())
    }

    fn content(&self, id: &RemoteId, version: &str) -> PathBuf {
        self.dir.join("content").join(key(id, version))
    }

    fn local_file(&self, n: u64) -> PathBuf {
        self.dir.join("local").join(n.to_string())
    }

    fn ino(&self, id: &NodeId) -> u64 {
        let ino = ino_for(id);
        held(&self.inodes).entry(ino).or_insert_with(|| id.clone());
        ino
    }

    fn top(&self) -> Node {
        Node {
            id: NodeId::Top,
            parent: NodeId::Top,
            name: String::new(),
            kind: NodeKind::Folder,
            size: 0,
            modified: None,
        }
    }

    fn entry(&self, id: &RemoteId) -> Result<Option<Entry>, DriveError> {
        Ok(self.listing.entry(id)?)
    }

    fn local_row(&self, n: u64) -> Option<LocalRow> {
        held(&self.index)
            .local
            .iter()
            .find(|row| row.n == n)
            .cloned()
    }

    fn attrs_of_entry(entry: &Entry) -> Attrs {
        let kind = NodeKind::of(&entry.kind);
        let size = match &kind {
            NodeKind::Online { url, .. } => online::link_file(url, &entry.id).len() as u64,
            NodeKind::Folder => 0,
            NodeKind::File => entry.size.0,
        };
        Attrs {
            kind,
            size,
            modified: entry.modified,
        }
    }

    fn attrs(&self, id: &NodeId) -> Result<Option<Attrs>, DriveError> {
        Ok(match id {
            NodeId::Top => Some(Attrs {
                kind: NodeKind::Folder,
                size: 0,
                modified: None,
            }),
            NodeId::Remote(id) => self.entry(id)?.as_ref().map(Self::attrs_of_entry),
            NodeId::Local(n) => self.local_row(*n).map(|row| Attrs {
                kind: NodeKind::File,
                size: row.size,
                modified: Some(UnixSeconds(row.made)),
            }),
        })
    }

    fn children(&self, folder: &NodeId) -> Result<Vec<Node>, DriveError> {
        let parent = match folder {
            NodeId::Top => None,
            NodeId::Remote(id) => match self.entry(id)? {
                Some(entry) if entry.kind == ItemKind::Folder => Some(id.clone()),
                _ => return Ok(Vec::new()),
            },
            NodeId::Local(_) => return Ok(Vec::new()),
        };
        let mut entries = self.listing.children(parent.as_ref())?;
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        let mut locals: Vec<LocalRow> = held(&self.index)
            .local
            .iter()
            .filter(|row| row.parent.as_deref() == parent.as_ref().map(|p| p.0.as_str()))
            .cloned()
            .collect();
        locals.sort_by_key(|row| row.n);
        let bases: Vec<String> = entries
            .iter()
            .map(|entry| naming::base_of(&entry.name, &entry.kind))
            .chain(locals.iter().map(|row| naming::shown_base(&row.name)))
            .collect();
        let names = naming::assign(&bases);
        let mut nodes = Vec::with_capacity(names.len());
        let mut names = names.into_iter();
        for entry in &entries {
            let attrs = Self::attrs_of_entry(entry);
            nodes.push(Node {
                id: NodeId::Remote(entry.id.clone()),
                parent: folder.clone(),
                name: names.next().unwrap_or_default(),
                kind: attrs.kind,
                size: attrs.size,
                modified: attrs.modified,
            });
        }
        for row in &locals {
            nodes.push(Node {
                id: NodeId::Local(row.n),
                parent: folder.clone(),
                name: names.next().unwrap_or_default(),
                kind: NodeKind::File,
                size: row.size,
                modified: Some(UnixSeconds(row.made)),
            });
        }
        for node in &nodes {
            self.ino(&node.id);
        }
        Ok(nodes)
    }

    fn parent_of(&self, id: &NodeId) -> Result<Option<NodeId>, DriveError> {
        Ok(match id {
            NodeId::Top => Some(NodeId::Top),
            NodeId::Remote(id) => self
                .entry(id)?
                .map(|entry| entry.parent.map_or(NodeId::Top, NodeId::Remote)),
            NodeId::Local(n) => self.local_row(*n).map(|row| {
                row.parent
                    .map_or(NodeId::Top, |p| NodeId::Remote(RemoteId(p)))
            }),
        })
    }

    fn node(&self, id: &NodeId) -> Result<Option<Node>, DriveError> {
        if *id == NodeId::Top {
            return Ok(Some(self.top()));
        }
        let Some(parent) = self.parent_of(id)? else {
            return Ok(None);
        };
        Ok(self
            .children(&parent)?
            .into_iter()
            .find(|node| node.id == *id))
    }

    /// Whether `id` is kept, itself or by a folder above it, as `index` says.
    fn kept_in(&self, index: &Index, id: &RemoteId) -> bool {
        index.kept.contains(&id.0)
            || self
                .listing
                .ancestors(id)
                .unwrap_or_default()
                .iter()
                .any(|above| index.kept.contains(&above.0))
    }

    /// Whether a folder above `id` is kept.
    fn kept_by_folder(&self, id: &NodeId) -> bool {
        let index = held(&self.index);
        let above = match id {
            NodeId::Top => return false,
            NodeId::Remote(id) => self.listing.ancestors(id).unwrap_or_default(),
            NodeId::Local(n) => match index.local.iter().find(|row| row.n == *n) {
                Some(LocalRow {
                    parent: Some(parent),
                    ..
                }) => {
                    let parent = RemoteId(parent.clone());
                    let mut above = self.listing.ancestors(&parent).unwrap_or_default();
                    above.insert(0, parent);
                    above
                }
                _ => Vec::new(),
            },
        };
        above.iter().any(|folder| index.kept.contains(&folder.0))
    }

    fn is_open(&self, id: &NodeId) -> bool {
        held(&self.open).get(id).is_some_and(|n| *n > 0)
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
        if let Err(why) = inner.flush_now() {
            eprintln!("porter-drive: the cache's index could not be written: {why}");
        }
    }
}
