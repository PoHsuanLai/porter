//! The cache's index, `index.json` in the cache directory (written atomically through porter-fs,
//! with the copy from before its last change beside it): per item the version cached, its size,
//! the ranges that are there and when it was last opened; the items kept downloaded; and the
//! files made on this computer (Save a Copy's copies).

use crate::ranges::Ranges;
use porter_fs::atomic::AtomicFile;
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

/// One file made on this computer and not on the drive yet.
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

/// The whole index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Index {
    /// The cached items by id.
    #[serde(default)]
    pub items: BTreeMap<String, Row>,
    /// The items kept downloaded by id (a folder keeps everything under it).
    #[serde(default)]
    pub kept: BTreeSet<String>,
    /// The files made here.
    #[serde(default)]
    pub local: Vec<LocalRow>,
    /// The next number a file made here takes.
    #[serde(default)]
    pub next_local: u64,
    /// The open clock.
    #[serde(default)]
    pub clock: u64,
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
        read(self.0.path())
            .or_else(|| {
                read(self.0.backup_path()).map(|mut index| {
                    index.items.clear();
                    index
                })
            })
            .unwrap_or_default()
    }

    /// Writes `index` whole.
    pub fn save(&self, index: &Index) -> std::io::Result<()> {
        let text = serde_json::to_string(index).map_err(std::io::Error::other)?;
        self.0.write(&text)
    }
}
