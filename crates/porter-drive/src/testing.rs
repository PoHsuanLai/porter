//! A drive held in memory: a [`MemoryListing`] of entries and a [`MemorySource`] of their bytes,
//! which can be taken offline and made to fetch one chunk at a time. For tests only.

use crate::listing::{Entry, Listing, ListingError};
use crate::source::{Fetching, Source, SourceError};
use porter_core::{Bytes, UnixSeconds, WebUrl};
use porter_sync::{ItemKind, RemoteId, RemoteVersion};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Semaphore;

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A listing in memory.
#[derive(Debug, Default)]
pub struct MemoryListing {
    entries: Mutex<BTreeMap<RemoteId, Entry>>,
}

impl MemoryListing {
    /// Puts `entry` in (or replaces the one with its id).
    pub fn put(&self, entry: Entry) {
        held(&self.entries).insert(entry.id.clone(), entry);
    }

    /// Takes `id` out.
    pub fn remove(&self, id: &str) {
        held(&self.entries).remove(&RemoteId(id.to_owned()));
    }

    /// A folder `id` named `name` in `parent`.
    pub fn folder(&self, id: &str, parent: Option<&str>, name: &str) {
        self.put(entry(id, parent, name, ItemKind::Folder, 0, "1"));
    }

    /// A file `id` named `name` in `parent`, `size` bytes, at `version`.
    pub fn file(&self, id: &str, parent: Option<&str>, name: &str, size: u64, version: &str) {
        self.put(entry(id, parent, name, ItemKind::File, size, version));
    }

    /// An online document `id` that opens at `url`, saved as `export`.
    pub fn online(
        &self,
        id: &str,
        parent: Option<&str>,
        name: &str,
        url: &str,
        export: Option<&str>,
    ) {
        let kind = ItemKind::Online {
            url: WebUrl::parse(url).expect("a test url"),
            export: export.map(str::to_owned),
        };
        self.put(entry(id, parent, name, kind, 0, "1"));
    }
}

/// An entry of a test listing.
pub fn entry(
    id: &str,
    parent: Option<&str>,
    name: &str,
    kind: ItemKind,
    size: u64,
    version: &str,
) -> Entry {
    Entry {
        id: RemoteId(id.to_owned()),
        parent: parent.map(|p| RemoteId(p.to_owned())),
        name: name.to_owned(),
        kind,
        size: Bytes(size),
        version: RemoteVersion(version.to_owned()),
        modified: Some(UnixSeconds(1_700_000_000)),
    }
}

impl Listing for MemoryListing {
    fn entry(&self, id: &RemoteId) -> Result<Option<Entry>, ListingError> {
        Ok(held(&self.entries).get(id).cloned())
    }

    fn children(&self, folder: Option<&RemoteId>) -> Result<Vec<Entry>, ListingError> {
        Ok(held(&self.entries)
            .values()
            .filter(|entry| entry.parent.as_ref() == folder)
            .cloned()
            .collect())
    }
}

/// Bytes in memory, by id.
#[derive(Debug, Default)]
pub struct MemorySource {
    bytes: Mutex<HashMap<RemoteId, Vec<u8>>>,
    offline: AtomicBool,
    gate: Mutex<Option<Arc<Semaphore>>>,
    fetched: Mutex<Vec<(String, u64, u64)>>,
    waiting: std::sync::atomic::AtomicUsize,
}

impl MemorySource {
    /// `bytes` as the content of `id`.
    pub fn put(&self, id: &str, bytes: Vec<u8>) {
        held(&self.bytes).insert(RemoteId(id.to_owned()), bytes);
    }

    /// Takes the drive offline, or back.
    pub fn set_offline(&self, offline: bool) {
        self.offline.store(offline, Ordering::SeqCst);
    }

    /// Holds every fetch until [`MemorySource::allow`] lets it through.
    pub fn hold(&self) {
        *held(&self.gate) = Some(Arc::new(Semaphore::new(0)));
    }

    /// Lets `n` held fetches through.
    pub fn allow(&self, n: usize) {
        if let Some(gate) = held(&self.gate).as_ref() {
            gate.add_permits(n);
        }
    }

    /// Lets every fetch through from now on.
    pub fn release(&self) {
        if let Some(gate) = held(&self.gate).take() {
            gate.add_permits(Semaphore::MAX_PERMITS / 2);
        }
    }

    /// How many fetches wait at the gate now.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    /// Each fetch so far: the id, the start and the length asked.
    pub fn fetched(&self) -> Vec<(String, u64, u64)> {
        held(&self.fetched).clone()
    }
}

impl Source for MemorySource {
    fn fetch<'a>(&'a self, id: &'a RemoteId, start: u64, len: u64) -> Fetching<'a> {
        Box::pin(async move {
            let gate = held(&self.gate).clone();
            if let Some(gate) = gate {
                self.waiting.fetch_add(1, Ordering::SeqCst);
                if let Ok(permit) = gate.acquire().await {
                    permit.forget();
                }
                self.waiting.fetch_sub(1, Ordering::SeqCst);
            }
            if self.offline.load(Ordering::SeqCst) {
                return Err(SourceError::Offline);
            }
            held(&self.fetched).push((id.0.clone(), start, len));
            let bytes = held(&self.bytes);
            let content = bytes.get(id).ok_or(SourceError::Refused)?;
            let from = usize::try_from(start)
                .unwrap_or(usize::MAX)
                .min(content.len());
            let to = from
                .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
                .min(content.len());
            Ok(content[from..to].to_vec())
        })
    }
}
