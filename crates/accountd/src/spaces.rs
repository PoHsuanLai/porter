//! The registry of desktop-wide Spaces (lane spaces): what `org.quire.Spaces1` serves, kept in
//! `spaces.json` beside `registry.json` with the same atomic save and `.bak` ([`AtomicFile`]).
//!
//! - Ids are minted here from a counter (`space-<n>`), never from the name, so a rename keeps
//!   the id. An id that is taken (an adopted one) is skipped.
//! - First start (no `spaces.json` yet): every desktop-wide Space a stored grant is scoped to
//!   (`Only(<slug>)`, ids from before Spaces were per app) is adopted as a Space of that id,
//!   named by its slug, with the default look; the file is written then, so a later start adopts
//!   nothing again.
//! - `Create` is limited per app to [`CREATES_PER_WINDOW`] within [`CREATE_WINDOW`], so a
//!   faulty app cannot flood the list.
//! - A `spaces.json` that cannot be read is never written over: the list is served empty and
//!   every change is refused until the person mends or moves the file.

use porter_core::consent::Grant;
use porter_core::{
    AppName, DesktopSpace, DesktopSpaceRecord, SpaceKind, SpaceLook, SpaceName, SpaceScope,
    UnixSeconds,
};
use porter_fs::atomic::AtomicFile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The file's name inside the state directory.
const FILE: &str = "spaces.json";
/// The file's own format version.
const VERSION: u16 = 1;

/// The most Spaces one app may make within [`CREATE_WINDOW`].
pub const CREATES_PER_WINDOW: usize = 10;
/// The window [`CREATES_PER_WINDOW`] counts over.
pub const CREATE_WINDOW: Duration = Duration::from_secs(60);

/// Where accountd keeps the desktop-wide Spaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SpacesStore {
    /// Only in memory: gone when accountd stops (tests, and a host with no state directory).
    #[default]
    Memory,
    /// `spaces.json` in this directory (the registry's state directory).
    File(PathBuf),
}

/// `spaces.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Document {
    version: u16,
    /// The number the next minted id takes.
    next: u64,
    /// The Spaces, in the order they were made.
    spaces: Vec<DesktopSpaceRecord>,
}

impl Document {
    fn empty() -> Self {
        Self {
            version: VERSION,
            next: 1,
            spaces: Vec::new(),
        }
    }

    fn find_mut(&mut self, id: &DesktopSpace) -> Option<&mut DesktopSpaceRecord> {
        self.spaces.iter_mut().find(|s| s.id == *id)
    }
}

/// Why a change to the Spaces was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpaceFault {
    /// No Space has that id.
    NotThere,
    /// The app made [`CREATES_PER_WINDOW`] Spaces within the window already.
    TooMany,
    /// The file could not be written, or was refused at start.
    Unsaved,
}

/// Whether changes may be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writable {
    Yes,
    /// The file on disk could not be read at start; it is left as it is.
    Refused,
}

/// The Spaces as accountd holds them.
#[derive(Debug)]
pub(crate) struct SpaceBook {
    doc: Document,
    file: Option<AtomicFile>,
    writable: Writable,
    /// When each app last made Spaces, within the window.
    creates: BTreeMap<AppName, VecDeque<Instant>>,
}

/// The desktop-wide Spaces the stored grants are scoped to, in id order.
fn legacy(grants: &[Grant]) -> BTreeSet<DesktopSpace> {
    grants
        .iter()
        .filter_map(|g| match &g.key.space {
            SpaceScope::Only(space) => match space.kind() {
                SpaceKind::Linked(desktop) => Some(desktop),
                SpaceKind::Outside | SpaceKind::App { .. } => None,
            },
            SpaceScope::Any => None,
        })
        .collect()
}

/// A Space adopted from a stored id: named by its slug, the default look.
fn adopted(id: DesktopSpace, now: UnixSeconds) -> Option<DesktopSpaceRecord> {
    Some(DesktopSpaceRecord {
        name: SpaceName::parse(id.as_str()).ok()?,
        id,
        look: SpaceLook::default(),
        created: now,
    })
}

impl SpaceBook {
    /// The Spaces `store` holds; on a first start (no file, or memory) the ones `grants` are
    /// scoped to are adopted, and a file store writes them at once. A message for the log comes
    /// back when the file was refused or could not be written.
    pub(crate) async fn open(
        store: &SpacesStore,
        grants: &[Grant],
        now: UnixSeconds,
    ) -> (Self, Option<String>) {
        let file = match store {
            SpacesStore::Memory => None,
            SpacesStore::File(dir) => Some(AtomicFile::new(dir.clone(), FILE)),
        };
        let read = match &file {
            None => Ok(None),
            Some(file) => {
                let file = file.clone();
                tokio::task::spawn_blocking(move || file.read())
                    .await
                    .unwrap_or_else(|e| Err(std::io::Error::other(e)))
            }
        };
        let mut book = Self {
            doc: Document::empty(),
            file,
            writable: Writable::Yes,
            creates: BTreeMap::new(),
        };
        let parsed = read.map(|bytes| {
            bytes.map(|bytes| serde_json::from_slice::<Document>(&bytes).map_err(|e| e.to_string()))
        });
        match parsed {
            Ok(Some(Ok(doc))) if doc.version == VERSION => {
                book.doc = doc;
                (book, None)
            }
            Ok(Some(Ok(doc))) => {
                book.writable = Writable::Refused;
                let why = format!("it is of a newer version ({})", doc.version);
                let said = book.refused_message(&why);
                (book, Some(said))
            }
            Ok(Some(Err(why))) => {
                book.writable = Writable::Refused;
                let said = book.refused_message(&why);
                (book, Some(said))
            }
            Err(why) => {
                book.writable = Writable::Refused;
                let said = book.refused_message(&why.to_string());
                (book, Some(said))
            }
            Ok(None) => {
                let mut doc = Document::empty();
                doc.spaces = legacy(grants)
                    .into_iter()
                    .filter_map(|id| adopted(id, now))
                    .collect();
                let said = book.commit(doc).await.err().map(|_| {
                    "the Spaces file could not be written; it is tried again on the next change"
                        .to_owned()
                });
                (book, said)
            }
        }
    }

    fn refused_message(&self, why: &str) -> String {
        let path = self
            .file
            .as_ref()
            .map(|f| f.path().display().to_string())
            .unwrap_or_default();
        format!(
            "the Spaces file {path} cannot be read ({why}). It was left as it is; no Space can be \
             made or changed until it is mended or moved away."
        )
    }

    /// Every Space, in the order they were made.
    pub(crate) fn list(&self) -> &[DesktopSpaceRecord] {
        &self.doc.spaces
    }

    /// Whether `app` may make another Space at `now`; counts it when it may.
    fn admit(&mut self, app: &AppName, now: Instant) -> bool {
        let made = self.creates.entry(app.clone()).or_default();
        while made
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= CREATE_WINDOW)
        {
            made.pop_front();
        }
        let room = made.len() < CREATES_PER_WINDOW;
        if room {
            made.push_back(now);
        }
        room
    }

    /// Makes a Space for `app`, and answers its new id.
    pub(crate) async fn create(
        &mut self,
        app: &AppName,
        name: SpaceName,
        look: SpaceLook,
        now: (Instant, UnixSeconds),
    ) -> Result<DesktopSpace, SpaceFault> {
        if self.writable == Writable::Refused {
            return Err(SpaceFault::Unsaved);
        }
        if !self.admit(app, now.0) {
            return Err(SpaceFault::TooMany);
        }
        let mut doc = self.doc.clone();
        // `next` comes from a file: one at the top of the range is a full counter (refused), not
        // an overflow.
        let (n, id) = (doc.next..=u64::MAX)
            .filter_map(|n| Some((n, DesktopSpace::parse(&format!("space-{n}")).ok()?)))
            .find(|(_, id)| !doc.spaces.iter().any(|s| s.id == *id))
            .ok_or(SpaceFault::Unsaved)?;
        doc.next = n.checked_add(1).ok_or(SpaceFault::Unsaved)?;
        doc.spaces.push(DesktopSpaceRecord {
            id: id.clone(),
            name,
            look,
            created: now.1,
        });
        self.commit(doc).await.map(|()| id)
    }

    /// Renames a Space.
    pub(crate) async fn rename(
        &mut self,
        id: &DesktopSpace,
        name: SpaceName,
    ) -> Result<(), SpaceFault> {
        self.change(id, |record| record.name = name).await
    }

    /// Gives a Space a new look.
    pub(crate) async fn set_look(
        &mut self,
        id: &DesktopSpace,
        look: SpaceLook,
    ) -> Result<(), SpaceFault> {
        self.change(id, |record| record.look = look).await
    }

    /// Removes a Space.
    pub(crate) async fn remove(&mut self, id: &DesktopSpace) -> Result<(), SpaceFault> {
        let mut doc = self.doc.clone();
        let before = doc.spaces.len();
        doc.spaces.retain(|s| s.id != *id);
        if doc.spaces.len() == before {
            return Err(SpaceFault::NotThere);
        }
        self.commit(doc).await
    }

    async fn change(
        &mut self,
        id: &DesktopSpace,
        edit: impl FnOnce(&mut DesktopSpaceRecord),
    ) -> Result<(), SpaceFault> {
        let mut doc = self.doc.clone();
        edit(doc.find_mut(id).ok_or(SpaceFault::NotThere)?);
        self.commit(doc).await
    }

    /// Saves `doc` and, once it is on disk, holds it. In memory it is held at once.
    async fn commit(&mut self, doc: Document) -> Result<(), SpaceFault> {
        if self.writable == Writable::Refused {
            return Err(SpaceFault::Unsaved);
        }
        if let Some(file) = self.file.clone() {
            let text = serde_json::to_string_pretty(&doc).map_err(|_| SpaceFault::Unsaved)?;
            tokio::task::spawn_blocking(move || file.write(&text))
                .await
                .map_err(|_| SpaceFault::Unsaved)?
                .map_err(|_| SpaceFault::Unsaved)?;
        }
        self.doc = doc;
        Ok(())
    }
}

/// Seconds since the Unix epoch now, for a Space's `created`.
pub(crate) fn unix_now() -> UnixSeconds {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    UnixSeconds(i64::try_from(secs).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests;
