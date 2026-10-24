//! The cache's index, `index.json` in the cache directory (written atomically through porter-fs,
//! owner-only, with the copy from before its last change beside it): per item the version
//! cached, its size, the ranges that are there and when it was last opened; the items kept
//! downloaded; and the changes made on this computer that the drive does not have yet
//! ([`Edit`]: files and folders made here, new bytes, new names and places, removals), which
//! are the queue of what is sent.

use crate::drive::NodeId;
use crate::ranges::Ranges;
use porter_fs::atomic::AtomicFile;
use porter_sync::Session;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The index file's name in the cache directory.
const INDEX: &str = "index.json";

/// One cached item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Row {
    /// The drive's version the bytes are of.
    pub version: String,
    /// Its size at that version.
    pub size: u64,
    /// The ranges that are in the file.
    pub present: Ranges,
    /// When it was last opened, as the index's own clock counts: oldest is evicted first.
    pub opened: u64,
    /// Every byte is there and was checked against the drive's hash (or its version): the file
    /// is the drive's at `version`. A row of an index from before the check is not, and is
    /// checked when next opened.
    #[serde(default)]
    pub checked: bool,
}

impl Row {
    /// Whether every byte is there, checked.
    pub fn whole(&self) -> bool {
        self.checked && self.present.covers(0, self.size)
    }
}

/// One file made on this computer, as an index from before changes were sent kept it (read and
/// turned into an [`Edit`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LocalRow {
    /// Its number, which names its file.
    pub n: u64,
    /// The drive's folder it is in; `None` at the top.
    pub parent: Option<String>,
    /// Its name.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// When it was made.
    pub made: i64,
}

/// The drive's item as this computer last knew it: what a change of it was made against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Base {
    /// Its version.
    pub version: String,
    /// Its folder; `None` at the top.
    pub parent: Option<String>,
    /// Its name.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// When it last changed.
    #[serde(default)]
    pub modified: Option<i64>,
}

/// Bytes written here and not sent: the file `local/<file>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Dirty {
    /// The number that names its file.
    pub file: u64,
    /// Its size.
    pub size: u64,
    /// When it was last written.
    pub modified: i64,
    /// How many times it was written: an upload of an older count leaves it dirty.
    #[serde(default)]
    pub writes: u64,
}

/// A change sent and not in the listing yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Awaiting {
    /// The version the listing showed before it was sent (`None`: it listed nothing), which it
    /// shows until it has caught up.
    pub before: Option<String>,
    /// Listing passes since that found nothing of an item new to it.
    #[serde(default)]
    pub passes: u32,
}

/// One item changed or made on this computer, as it is here and as the drive last had it.
/// Changes coalesce: an item has one edit whatever was done to it, and what is sent is the
/// difference between the two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Edit {
    /// The item: one of the drive's, or one made here (until the drive gives it an id).
    pub id: NodeId,
    /// Whether it is a folder.
    pub folder: bool,
    /// The folder it is in here.
    pub parent: NodeId,
    /// Its name here, as the drive is to have it.
    pub name: String,
    /// The drive's item as last known; `None` for one made here that the drive has not got.
    pub base: Option<Base>,
    /// Bytes written here and not sent.
    pub dirty: Option<Dirty>,
    /// Removed here: it goes to the drive's recycle bin.
    #[serde(default)]
    pub removed: bool,
    /// The upload session in flight, resumed after a restart. A credential: the index is
    /// owner-only, and the session never shows in a log.
    #[serde(default)]
    pub session: Option<Session>,
    /// Sent, and not in the listing yet.
    #[serde(default)]
    pub awaiting: Option<Awaiting>,
    /// When it was made here.
    pub made: i64,
    /// Its place in the queue: the oldest change is sent first.
    pub seq: u64,
    /// The drive refused it (no room, no right to change it): the words, until it is tried
    /// again (the next listing change, or a new change of it).
    #[serde(default)]
    pub refused: Option<String>,
}

impl Edit {
    /// A file or folder made here in `parent`, named `name`, with its bytes (a file's) in
    /// `local/<file>`.
    pub fn made_here(n: u64, parent: NodeId, name: String, file: Option<Dirty>, made: i64) -> Self {
        Self {
            id: NodeId::Local(n),
            folder: file.is_none(),
            parent,
            name,
            base: None,
            dirty: file,
            removed: false,
            session: None,
            awaiting: None,
            made,
            seq: 0,
            refused: None,
        }
    }

    /// Whether the drive's item still stands where it was and as it was named.
    pub fn in_place(&self) -> bool {
        self.base.as_ref().is_some_and(|base| {
            base.name == self.name
                && match &self.parent {
                    NodeId::Top => base.parent.is_none(),
                    NodeId::Remote(id) => base.parent.as_deref() == Some(id.0.as_str()),
                    NodeId::Local(_) => false,
                }
        })
    }

    /// Whether something of it is still to be sent (a removal is sent once).
    pub fn pending(&self) -> bool {
        match (&self.base, self.removed) {
            (None, _) => true,
            (Some(_), true) => self.awaiting.is_none(),
            (Some(_), false) => self.dirty.is_some() || !self.in_place(),
        }
    }
}

/// The whole index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Index {
    /// The cached items by id.
    #[serde(default)]
    pub items: BTreeMap<String, Row>,
    /// The items kept downloaded by id (a folder keeps everything under it).
    #[serde(default)]
    pub kept: BTreeSet<String>,
    /// The files made here, as an index from before changes were sent kept them.
    #[serde(default, skip_serializing)]
    pub local: Vec<LocalRow>,
    /// The changes made here, by item.
    #[serde(default, with = "edits")]
    pub edits: BTreeMap<NodeId, Edit>,
    /// What an item made here became: the id the drive gave it, or the file it was saved over.
    #[serde(default, with = "aliases")]
    pub aliases: BTreeMap<u64, NodeId>,
    /// The inode number an item keeps that changed its id ([`crate::drive::key_text`] names it).
    #[serde(default)]
    pub inos: BTreeMap<String, u64>,
    /// Two versions of one file kept side by side: each names the other.
    #[serde(default, with = "pairs")]
    pub conflicts: BTreeMap<NodeId, NodeId>,
    /// The next number a file made here takes.
    #[serde(default)]
    pub next_local: u64,
    /// The next place in the queue.
    #[serde(default)]
    pub next_seq: u64,
    /// The open clock.
    #[serde(default)]
    pub clock: u64,
}

impl Index {
    /// Takes the files made here that an index from before the queue kept in as changes to
    /// send.
    fn migrate(&mut self) {
        for row in std::mem::take(&mut self.local) {
            let parent = row
                .parent
                .map_or(NodeId::Top, |p| NodeId::Remote(porter_sync::RemoteId(p)));
            let dirty = Dirty {
                file: row.n,
                size: row.size,
                modified: row.made,
                writes: 0,
            };
            let mut edit = Edit::made_here(row.n, parent, row.name, Some(dirty), row.made);
            edit.seq = self.take_seq();
            self.next_local = self.next_local.max(row.n + 1);
            self.edits.insert(edit.id.clone(), edit);
        }
    }

    /// A new number for a file made here.
    pub fn take_local(&mut self) -> u64 {
        let n = self.next_local.max(1);
        self.next_local = n + 1;
        n
    }

    /// The next place in the queue.
    pub fn take_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }
}

/// The edits as a list (JSON has string keys only).
mod edits {
    use super::{Edit, NodeId};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(map: &BTreeMap<NodeId, Edit>, s: S) -> Result<S::Ok, S::Error> {
        map.values().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<NodeId, Edit>, D::Error> {
        let list = Vec::<Edit>::deserialize(d)?;
        Ok(list.into_iter().map(|e| (e.id.clone(), e)).collect())
    }
}

/// The aliases as a list of pairs.
mod aliases {
    use super::NodeId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(map: &BTreeMap<u64, NodeId>, s: S) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<u64, NodeId>, D::Error> {
        Ok(Vec::<(u64, NodeId)>::deserialize(d)?.into_iter().collect())
    }
}

/// The conflict pairs as a list.
mod pairs {
    use super::NodeId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<NodeId, NodeId>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<NodeId, NodeId>, D::Error> {
        Ok(Vec::<(NodeId, NodeId)>::deserialize(d)?
            .into_iter()
            .collect())
    }
}

/// The index file of a cache directory.
#[derive(Debug, Clone)]
pub(crate) struct IndexFile(AtomicFile);

impl IndexFile {
    pub fn new(dir: &Path) -> Self {
        Self(AtomicFile::new(dir.to_path_buf(), INDEX))
    }

    /// The index as stored, or an empty one. When the file is unreadable, the copy before it,
    /// without its cached items: what it says is kept and what was made here stands, but it
    /// cannot vouch for the content files, written to after it, so it claims none of them.
    pub fn load(&self) -> Index {
        let read = |path: PathBuf| {
            std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
        };
        let mut index = read(self.0.path())
            .or_else(|| {
                read(self.0.backup_path()).map(|mut index| {
                    index.items.clear();
                    index
                })
            })
            .unwrap_or_default();
        index.migrate();
        index
    }

    /// Writes `index` whole.
    pub fn save(&self, index: &Index) -> std::io::Result<()> {
        let text = serde_json::to_string(index).map_err(std::io::Error::other)?;
        self.0.write(&text)
    }
}
