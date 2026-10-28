//! A drive held in memory: a [`MemoryListing`] of entries and a [`MemorySource`] of their bytes,
//! which can be taken offline and made to fetch one chunk at a time, and a [`MemorySink`] that
//! takes changes into both. For tests only.

use crate::listing::{Entry, Listing, ListingError};
use crate::sink::{Sending, Sink};
use crate::source::{CopyError, Exporting, Fetching, Source, SourceError};
use porter_capability::HashKind;
use porter_ids::{Bytes, UnixSeconds, WebUrl};
use porter_sync::{
    BaseVersion, Conflict, ContentHash, Found, ItemKind, Place, PutRefused, RemoteId, RemoteSide,
    RemoteVersion, RetryAfter, Session, Upload, UploadTo, Written,
};
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
    unreadable: AtomicBool,
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

    /// Makes every read fail (`false`: read again).
    pub fn set_unreadable(&self, unreadable: bool) {
        self.unreadable.store(unreadable, Ordering::SeqCst);
    }

    fn readable(&self) -> Result<(), ListingError> {
        match self.unreadable.load(Ordering::SeqCst) {
            true => Err(ListingError("the test listing is unreadable".to_owned())),
            false => Ok(()),
        }
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
        hash: None,
        modified: Some(UnixSeconds(1_700_000_000)),
    }
}

impl Listing for MemoryListing {
    fn entry(&self, id: &RemoteId) -> Result<Option<Entry>, ListingError> {
        self.readable()?;
        Ok(held(&self.entries).get(id).cloned())
    }

    fn children(&self, folder: Option<&RemoteId>) -> Result<Vec<Entry>, ListingError> {
        self.readable()?;
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
    exports: Mutex<HashMap<RemoteId, Result<Vec<u8>, CopyError>>>,
    hashes: Mutex<Option<HashKind>>,
}

impl MemorySource {
    /// `bytes` as the content of `id`.
    pub fn put(&self, id: &str, bytes: Vec<u8>) {
        held(&self.bytes).insert(RemoteId(id.to_owned()), bytes);
    }

    /// `bytes` as what the online document `id` exports as.
    pub fn put_export(&self, id: &str, bytes: Vec<u8>) {
        held(&self.exports).insert(RemoteId(id.to_owned()), Ok(bytes));
    }

    /// The online document `id`'s export refused with `why`. (One with no export put is
    /// refused `NoFormat`.)
    pub fn refuse_export(&self, id: &str, why: CopyError) {
        held(&self.exports).insert(RemoteId(id.to_owned()), Err(why));
    }

    /// Says the listing's hashes are of `kind` (by default it has none).
    pub fn set_hashes(&self, kind: HashKind) {
        *held(&self.hashes) = Some(kind);
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

    fn hashes(&self) -> HashKind {
        held(&self.hashes).unwrap_or(HashKind::None)
    }

    fn export<'a>(&'a self, id: &'a RemoteId, mime: &'a str) -> Exporting<'a> {
        let _ = mime;
        Box::pin(async move {
            if self.offline.load(Ordering::SeqCst) {
                return Err(CopyError::Failed);
            }
            held(&self.exports)
                .get(id)
                .cloned()
                .unwrap_or(Err(CopyError::NoFormat))
        })
    }
}

/// One change a [`MemorySink`] took, as its log shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Took {
    /// A folder made: its parent and name.
    Folder(Option<String>, String),
    /// A new file uploaded: its parent, name and size.
    New(Option<String>, String, u64),
    /// New bytes for a file: its id and size.
    Replace(String, u64),
    /// A rename or move: the id, the new parent and name.
    Relocate(String, Option<String>, String),
    /// Sent to the recycle bin: the id.
    Trash(String),
}

/// How a [`MemorySink`] refuses every change, while it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refuse {
    /// The drive could not be reached.
    Offline,
    /// No room left.
    Quota,
    /// The account may not write.
    Forbidden,
    /// The account's sign-in is refused.
    SignedOut,
    /// The item is in use on the drive.
    InUse,
    /// The drive declines, for a reason of its own.
    Declined,
}

#[derive(Debug, Default)]
struct SinkState {
    took: Vec<Took>,
    refuse: Option<Refuse>,
    names: Option<fn(&str) -> bool>,
    sessions: HashMap<String, Vec<u8>>,
    next: u64,
    cut_after: Option<u32>,
    cuts: u32,
    largest: u64,
    read: u64,
    bin: Vec<String>,
    lagging: Option<Vec<Option<Entry>>>,
    lag_removed: Vec<RemoteId>,
    gate: Option<Arc<Semaphore>>,
    abandoned: u32,
    uploads: Vec<Vec<u8>>,
    lose: bool,
    reserving: bool,
    path_max: Option<usize>,
    tries: u32,
    twins: bool,
}

/// The writes of a drive held in memory: what it takes changes a [`MemoryListing`] and a
/// [`MemorySource`], as a drive's next listing pass would show it. It can refuse, refuse names,
/// cut an upload off mid-way and hold its listing back.
#[derive(Debug)]
pub struct MemorySink {
    listing: Arc<MemoryListing>,
    source: Arc<MemorySource>,
    chunk: u64,
    state: Mutex<SinkState>,
}

impl MemorySink {
    /// Writes into `listing` and `source`, uploads `chunk` bytes at a time.
    pub fn new(listing: Arc<MemoryListing>, source: Arc<MemorySource>, chunk: u64) -> Self {
        Self {
            listing,
            source,
            chunk: chunk.max(1),
            state: Mutex::default(),
        }
    }

    /// Every change taken so far, in order.
    pub fn took(&self) -> Vec<Took> {
        held(&self.state).took.clone()
    }

    /// Refuses every change so (`None`: takes them again).
    pub fn refuse(&self, how: Option<Refuse>) {
        held(&self.state).refuse = how;
    }

    /// Takes only names `rule` takes.
    pub fn names(&self, rule: fn(&str) -> bool) {
        held(&self.state).names = Some(rule);
    }

    /// Cuts the next upload off after `chunks` chunks, as a dropped connection does.
    pub fn cut_after(&self, chunks: u32) {
        held(&self.state).cut_after = Some(chunks);
    }

    /// How many uploads were cut off.
    pub fn cuts(&self) -> u32 {
        held(&self.state).cuts
    }

    /// The most bytes one chunk of an upload read.
    pub fn largest_chunk(&self) -> u64 {
        held(&self.state).largest
    }

    /// The bytes uploads read in all.
    pub fn read_bytes(&self) -> u64 {
        held(&self.state).read
    }

    /// The names in the recycle bin.
    pub fn bin(&self) -> Vec<String> {
        held(&self.state).bin.clone()
    }

    /// The bytes of each upload it took, in order.
    pub fn uploads(&self) -> Vec<Vec<u8>> {
        held(&self.state).uploads.clone()
    }

    /// Takes the next change and loses its answer, as a connection dropped after the drive
    /// took it: the change says the drive could not be reached.
    pub fn lose_next(&self) {
        held(&self.state).lose = true;
    }

    /// Reserves ids for new items, as a drive that lets its client choose them does.
    pub fn reserving(&self) {
        held(&self.state).reserving = true;
    }

    /// Takes a second item of a name taken, as Google Drive does.
    pub fn twins(&self) {
        held(&self.state).twins = true;
    }

    /// Takes paths of at most `max` characters.
    pub fn set_path_max(&self, max: usize) {
        held(&self.state).path_max = Some(max);
    }

    /// How many steps were tried, taken or refused.
    pub fn tries(&self) -> u32 {
        held(&self.state).tries
    }

    /// The answer of a change taken: lost when [`MemorySink::lose_next`] asked.
    fn answer<T>(&self, taken: T) -> Result<T, PutRefused> {
        match std::mem::take(&mut held(&self.state).lose) {
            true => Err(PutRefused::Transient(RetryAfter(0))),
            false => Ok(taken),
        }
    }

    /// The id a new item at `place` is made under: the one reserved for it, unless something
    /// has it (a conflict, as a drive answers), else a new one.
    fn id_for(&self, place: &Place) -> Result<RemoteId, PutRefused> {
        match &place.id {
            Some(id) => match self.listing.entry(id).ok().flatten() {
                Some(there) => Err(PutRefused::Conflict(Conflict {
                    item: id.clone(),
                    base: BaseVersion::Absent,
                    remote: RemoteSide::Exists(id.clone(), there.version),
                })),
                None => Ok(id.clone()),
            },
            None => Ok(self.new_id()),
        }
    }

    /// How many upload sessions it was told to give up.
    pub fn abandoned(&self) -> u32 {
        held(&self.state).abandoned
    }

    /// Holds every upload after its first chunk until [`MemorySink::release`].
    pub fn hold(&self) {
        held(&self.state).gate = Some(Arc::new(Semaphore::new(0)));
    }

    /// Lets every held upload go on, and those after.
    pub fn release(&self) {
        if let Some(gate) = held(&self.state).gate.take() {
            gate.add_permits(Semaphore::MAX_PERMITS / 2);
        }
    }

    /// Holds what it takes out of the listing until [`MemorySink::catch_up`].
    pub fn lag(&self) {
        held(&self.state).lagging = Some(Vec::new());
    }

    /// Puts what it held back into the listing.
    pub fn catch_up(&self) {
        let (held_back, removed) = {
            let mut state = held(&self.state);
            (
                state.lagging.take().unwrap_or_default(),
                std::mem::take(&mut state.lag_removed),
            )
        };
        for entry in held_back.into_iter().flatten() {
            self.listing.put(entry);
        }
        for id in removed {
            self.listing.remove(&id.0);
        }
    }

    fn show(&self, entry: Entry) {
        let mut state = held(&self.state);
        match state.lagging.as_mut() {
            Some(lag) => lag.push(Some(entry)),
            None => {
                drop(state);
                self.listing.put(entry);
            }
        }
    }

    fn hide(&self, id: &RemoteId) {
        let mut state = held(&self.state);
        match state.lagging.is_some() {
            true => state.lag_removed.push(id.clone()),
            false => {
                drop(state);
                self.listing.remove(&id.0);
            }
        }
    }

    fn version(&self) -> RemoteVersion {
        let mut state = held(&self.state);
        state.next += 1;
        RemoteVersion(format!("w{}", state.next))
    }

    fn check(&self, name: Option<&str>) -> Result<(), PutRefused> {
        let mut state = held(&self.state);
        state.tries += 1;
        match state.refuse {
            Some(Refuse::Offline) => return Err(PutRefused::Transient(RetryAfter(0))),
            Some(Refuse::Quota) => return Err(PutRefused::Quota),
            Some(Refuse::Forbidden) => return Err(PutRefused::Forbidden),
            Some(Refuse::SignedOut) => return Err(PutRefused::Unauthorized),
            Some(Refuse::InUse) => return Err(PutRefused::InUse(RetryAfter(0))),
            Some(Refuse::Declined) => return Err(PutRefused::Declined),
            None => {}
        }
        match (name, state.names) {
            (Some(name), Some(rule)) if !rule(name) => Err(PutRefused::NameRefused),
            _ => Ok(()),
        }
    }

    fn at_base(&self, id: &RemoteId, base: &BaseVersion) -> Result<Entry, PutRefused> {
        let now = self.listing.entry(id).ok().flatten();
        match (now, base) {
            (Some(entry), BaseVersion::At(seen)) if entry.version == *seen => Ok(entry),
            (Some(entry), _) => Err(PutRefused::Conflict(Conflict {
                item: id.clone(),
                base: base.clone(),
                remote: RemoteSide::Changed(entry.version),
            })),
            (None, _) => Err(PutRefused::Conflict(Conflict {
                item: id.clone(),
                base: base.clone(),
                remote: RemoteSide::Deleted(RemoteVersion(String::new())),
            })),
        }
    }

    fn free(&self, place: &Place, except: Option<&RemoteId>) -> Result<(), PutRefused> {
        if held(&self.state).twins {
            return Ok(());
        }
        let taken = self
            .listing
            .children(place.parent.as_ref())
            .unwrap_or_default()
            .into_iter()
            .find(|e| e.name == place.name && Some(&e.id) != except);
        match taken {
            Some(entry) => Err(PutRefused::Conflict(Conflict {
                item: entry.id.clone(),
                base: BaseVersion::Absent,
                remote: RemoteSide::Exists(entry.id, entry.version),
            })),
            None => Ok(()),
        }
    }

    fn new_id(&self) -> RemoteId {
        let mut state = held(&self.state);
        state.next += 1;
        RemoteId(format!("made{}", state.next))
    }
}

fn parent_text(place: &Place) -> Option<String> {
    place.parent.as_ref().map(|p| p.0.clone())
}

impl Sink for MemorySink {
    fn accepts_name(&self, name: &str) -> bool {
        held(&self.state).names.is_none_or(|rule| rule(name))
    }

    fn path_max(&self) -> Option<usize> {
        held(&self.state).path_max
    }

    fn look<'a>(&'a self, item: &'a RemoteId) -> Sending<'a, Option<Found>> {
        Box::pin(async move {
            let Some(entry) = self.listing.entry(item).ok().flatten() else {
                return Ok(None);
            };
            let kind = self.source.hashes();
            let hash = held(&self.source.bytes)
                .get(item)
                .and_then(|bytes| crate::hash::of_bytes(kind, bytes))
                .map(ContentHash);
            Ok(Some(Found {
                version: entry.version,
                size: entry.size.0,
                hash,
                folder: entry.kind == ItemKind::Folder,
            }))
        })
    }

    fn reserve_ids(&self, count: usize) -> Sending<'_, Vec<RemoteId>> {
        Box::pin(async move {
            let reserving = held(&self.state).reserving;
            match reserving {
                true => Ok((0..count).map(|_| self.new_id()).collect()),
                false => Ok(Vec::new()),
            }
        })
    }

    fn make_folder<'a>(&'a self, place: &'a Place) -> Sending<'a, Written> {
        Box::pin(async move {
            self.check(Some(&place.name))?;
            let id = self.id_for(place)?;
            self.free(place, None)?;
            let version = self.version();
            let mut folder = entry(&id.0, None, &place.name, ItemKind::Folder, 0, &version.0);
            folder.parent = place.parent.clone();
            self.show(folder);
            held(&self.state)
                .took
                .push(Took::Folder(parent_text(place), place.name.clone()));
            self.answer(Written { id, version })
        })
    }

    fn upload<'a>(&'a self, upload: Upload<'a>) -> Sending<'a, Written> {
        Box::pin(async move {
            let name = match &upload.to {
                UploadTo::New(place) => Some(place.name.as_str()),
                UploadTo::Replace(_) => None,
            };
            self.check(name)?;
            let (id, mut file) = match &upload.to {
                UploadTo::New(place) => {
                    let id = self.id_for(place)?;
                    self.free(place, None)?;
                    let mut file = entry("", None, &place.name, ItemKind::File, 0, "");
                    file.parent = place.parent.clone();
                    (Some(id), file)
                }
                UploadTo::Replace(id) => (Some(id.clone()), self.at_base(id, &upload.base)?),
            };
            let size = upload.content.size();
            let mut bytes = Vec::new();
            let mut session = None;
            if size > self.chunk {
                let resumed = upload.resume.as_ref().and_then(|old| {
                    let had = held(&self.state).sessions.get(old.address()).cloned()?;
                    Some((old.address().to_owned(), had))
                });
                let key = match resumed {
                    Some((key, had)) => {
                        bytes = had;
                        key
                    }
                    None => {
                        let key = format!("mem://session/{}", self.new_id().0);
                        held(&self.state).sessions.insert(key.clone(), Vec::new());
                        upload.progress.session(Some(&Session::new(key.clone())));
                        key
                    }
                };
                session = Some(key);
            }
            let mut at = bytes.len() as u64;
            let mut chunks = 0u32;
            while at < size {
                if upload.progress.held() {
                    if let Some(key) = &session {
                        held(&self.state).sessions.insert(key.clone(), bytes);
                    }
                    return Err(PutRefused::Transient(RetryAfter(0)));
                }
                let cut = held(&self.state).cut_after;
                if cut.is_some_and(|after| chunks >= after) {
                    let mut state = held(&self.state);
                    state.cut_after = None;
                    state.cuts += 1;
                    if let Some(key) = &session {
                        state.sessions.insert(key.clone(), bytes);
                    }
                    return Err(PutRefused::Transient(RetryAfter(0)));
                }
                let chunk = upload
                    .content
                    .read(at, self.chunk)
                    .map_err(|_| PutRefused::Transient(RetryAfter(0)))?;
                {
                    let mut state = held(&self.state);
                    state.largest = state.largest.max(chunk.len() as u64);
                    state.read += chunk.len() as u64;
                }
                at += chunk.len() as u64;
                bytes.extend_from_slice(&chunk);
                chunks += 1;
                upload.progress.sent(at);
                tokio::task::yield_now().await;
                let gate = held(&self.state).gate.clone();
                if let Some(gate) = gate
                    && at < size
                {
                    let _through = gate.acquire().await;
                }
            }
            if let Some(key) = &session {
                held(&self.state).sessions.remove(key);
                upload.progress.session(None);
            }
            let id = id.unwrap_or_else(|| self.new_id());
            let version = self.version();
            file.id = id.clone();
            file.size = Bytes(size);
            file.version = version.clone();
            file.hash = None;
            held(&self.state).uploads.push(bytes.clone());
            self.source.put(&id.0, bytes);
            held(&self.state).took.push(match &upload.to {
                UploadTo::New(place) => Took::New(parent_text(place), place.name.clone(), size),
                UploadTo::Replace(id) => Took::Replace(id.0.clone(), size),
            });
            self.show(file);
            self.answer(Written { id, version })
        })
    }

    fn relocate<'a>(
        &'a self,
        item: &'a RemoteId,
        base: &'a BaseVersion,
        to: &'a Place,
    ) -> Sending<'a, Written> {
        Box::pin(async move {
            self.check(Some(&to.name))?;
            let mut moved = self.at_base(item, base)?;
            self.free(to, Some(item))?;
            let version = self.version();
            moved.parent = to.parent.clone();
            moved.name = to.name.clone();
            moved.version = version.clone();
            self.show(moved);
            held(&self.state).took.push(Took::Relocate(
                item.0.clone(),
                parent_text(to),
                to.name.clone(),
            ));
            Ok(Written {
                id: item.clone(),
                version,
            })
        })
    }

    fn abandon<'a>(
        &'a self,
        session: &'a Session,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let mut state = held(&self.state);
            if state.sessions.remove(session.address()).is_some() {
                state.abandoned += 1;
            }
        })
    }

    fn trash<'a>(&'a self, item: &'a RemoteId, base: &'a BaseVersion) -> Sending<'a, ()> {
        Box::pin(async move {
            self.check(None)?;
            let gone = self.at_base(item, base)?;
            let under = self.listing.descendants(item).unwrap_or_default();
            for inside in &under {
                self.hide(&inside.id);
            }
            self.hide(item);
            let mut state = held(&self.state);
            state.bin.push(gone.name);
            state.took.push(Took::Trash(item.0.clone()));
            Ok(())
        })
    }
}
