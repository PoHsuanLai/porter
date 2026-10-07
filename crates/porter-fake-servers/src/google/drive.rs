//! The Drive app data folder behind the fake Google: files in a tree below one root (the
//! `appDataFolder` alias names it), a change counter that `changes.list` reads, and the
//! resumable uploads in flight. Pure state; `drive_routes` is the HTTP over it.
//!
//! What it keeps from Drive: a file's `version` grows with every change to it; names are not
//! unique (two files may share one in a folder); `md5Checksum` is the real MD5 of the content;
//! a permanently deleted file is reported by `changes.list` as `removed` with no `file`, a
//! trashed one as a file with `trashed: true`; a page token older than the last expiry is
//! refused.

use super::md5::md5_hex;
use std::collections::BTreeMap;

/// The mime type of a folder.
pub const FOLDER: &str = "application/vnd.google-apps.folder";
/// The id the root answers to besides its own.
pub const ALIAS: &str = "appDataFolder";
/// What a non-final resumable chunk's size must be a multiple of (256 KiB).
pub const CHUNK_UNIT: usize = 262_144;

/// One file or folder.
#[derive(Debug, Clone)]
pub struct Node {
    /// Its id.
    pub id: String,
    /// Its parent's id; the root has none.
    pub parent: Option<String>,
    /// Its name.
    pub name: String,
    /// `None` for a folder.
    pub bytes: Option<Vec<u8>>,
    /// Counts its changes.
    pub version: u64,
    /// The change counter at its last change (what `changes.list` orders by).
    pub changed: u64,
    /// In the trash.
    pub trashed: bool,
    /// Permanently deleted: only the change log remembers it.
    pub removed: bool,
}

impl Node {
    /// Whether it is a folder.
    pub fn is_folder(&self) -> bool {
        self.bytes.is_none()
    }

    /// Whether a listing shows it.
    pub fn live(&self) -> bool {
        !self.trashed && !self.removed
    }
}

/// Why the drive refused a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The limit would be passed (`403 storageQuotaExceeded`).
    Full,
    /// The parent is not a folder, or does not exist (`404`).
    NoParent,
}

/// Where a resumable upload will put its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aim {
    /// A file that exists.
    Item(String),
    /// A new file in this folder.
    New {
        /// The parent folder's id.
        parent: String,
        /// The name.
        name: String,
    },
}

/// A resumable upload in flight.
#[derive(Debug, Clone)]
pub struct Upload {
    /// Where it ends.
    pub aim: Aim,
    /// The bytes received.
    pub received: Vec<u8>,
}

/// The drive.
#[derive(Debug)]
pub struct Drive {
    nodes: BTreeMap<String, Node>,
    next: u64,
    seq: u64,
    oldest_valid: u64,
    limit: Option<u64>,
    uploads: BTreeMap<String, Upload>,
    root: String,
}

impl Default for Drive {
    fn default() -> Self {
        let root = "0AFakeAppDataRoot".to_owned();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            root.clone(),
            Node {
                id: root.clone(),
                parent: None,
                name: ALIAS.to_owned(),
                bytes: None,
                version: 1,
                changed: 0,
                trashed: false,
                removed: false,
            },
        );
        Self {
            nodes,
            next: 1,
            seq: 0,
            oldest_valid: 0,
            limit: None,
            uploads: BTreeMap::new(),
            root,
        }
    }
}

impl Drive {
    /// The root's id.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// The id for an id or the alias.
    pub fn resolve<'a>(&'a self, id: &'a str) -> &'a str {
        if id == ALIAS { &self.root } else { id }
    }

    /// The change counter.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// A node by id or alias, removed ones too.
    pub fn any(&self, id: &str) -> Option<&Node> {
        self.nodes.get(self.resolve(id))
    }

    /// A node by id or alias that has not been permanently deleted.
    pub fn get(&self, id: &str) -> Option<&Node> {
        self.any(id).filter(|n| !n.removed)
    }

    /// The live children of a folder.
    pub fn children(&self, parent: &str) -> Vec<&Node> {
        let parent = self.resolve(parent);
        self.nodes
            .values()
            .filter(|n| n.live() && n.parent.as_deref() == Some(parent))
            .collect()
    }

    /// Every live node but the root, in id order.
    pub fn live(&self) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.live() && n.id != self.root)
            .collect()
    }

    /// The live node at `names` below the root.
    pub fn at(&self, names: &[String]) -> Option<&Node> {
        names.iter().try_fold(self.get(&self.root)?, |at, name| {
            self.children(&at.id).into_iter().find(|n| &n.name == name)
        })
    }

    /// The bytes in use: live files.
    pub fn used(&self) -> u64 {
        self.nodes
            .values()
            .filter(|n| n.live())
            .map(|n| n.bytes.as_ref().map_or(0, |b| b.len() as u64))
            .sum()
    }

    /// The limit, if any.
    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    /// Sets the limit.
    pub fn set_limit(&mut self, total: u64) {
        self.limit = Some(total);
    }

    fn touch(&mut self, id: &str) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(node) = self.nodes.get_mut(id) {
            node.version = seq;
            node.changed = seq;
        }
    }

    fn fresh_id(&mut self) -> String {
        self.next += 1;
        format!("1fake{:05}", self.next)
    }

    /// Makes a folder.
    pub fn make_folder(&mut self, parent: &str, name: &str) -> Result<String, Refused> {
        let parent = self.resolve(parent).to_owned();
        if !self.get(&parent).is_some_and(Node::is_folder) {
            return Err(Refused::NoParent);
        }
        let id = self.fresh_id();
        self.nodes.insert(
            id.clone(),
            Node {
                id: id.clone(),
                parent: Some(parent),
                name: name.to_owned(),
                bytes: None,
                version: 0,
                changed: 0,
                trashed: false,
                removed: false,
            },
        );
        self.touch(&id);
        Ok(id)
    }

    /// The folders on `names` below the root, made as needed.
    pub fn ensure(&mut self, names: &[String]) -> Result<String, Refused> {
        let mut at = self.root.clone();
        for name in names {
            let found = self
                .children(&at)
                .into_iter()
                .find(|n| n.is_folder() && &n.name == name)
                .map(|n| n.id.clone());
            at = match found {
                Some(id) => id,
                None => self.make_folder(&at, name)?,
            };
        }
        Ok(at)
    }

    fn room_for(&self, id: Option<&str>, len: usize) -> bool {
        let freed = id
            .and_then(|id| self.get(id))
            .and_then(|n| n.bytes.as_ref())
            .map_or(0, Vec::len) as u64;
        self.limit
            .is_none_or(|limit| self.used() - freed + len as u64 <= limit)
    }

    /// Creates a file in `parent`, or replaces the bytes of file `over`.
    pub fn write(
        &mut self,
        parent: &str,
        name: &str,
        bytes: Vec<u8>,
        over: Option<&str>,
    ) -> Result<String, Refused> {
        if !self.room_for(over, bytes.len()) {
            return Err(Refused::Full);
        }
        if let Some(id) = over.map(|id| self.resolve(id).to_owned()) {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.bytes = Some(bytes);
            }
            self.touch(&id);
            return Ok(id);
        }
        let parent = self.resolve(parent).to_owned();
        if !self.get(&parent).is_some_and(Node::is_folder) {
            return Err(Refused::NoParent);
        }
        let id = self.fresh_id();
        self.nodes.insert(
            id.clone(),
            Node {
                id: id.clone(),
                parent: Some(parent),
                name: name.to_owned(),
                bytes: Some(bytes),
                version: 0,
                changed: 0,
                trashed: false,
                removed: false,
            },
        );
        self.touch(&id);
        Ok(id)
    }

    /// Renames and/or moves a node.
    pub fn relocate(&mut self, id: &str, name: Option<&str>, parent: Option<&str>) -> bool {
        let id = self.resolve(id).to_owned();
        let parent = parent.map(|p| self.resolve(p).to_owned());
        if parent
            .as_deref()
            .is_some_and(|p| !self.get(p).is_some_and(Node::is_folder))
        {
            return false;
        }
        let Some(node) = self.nodes.get_mut(&id) else {
            return false;
        };
        if let Some(name) = name {
            node.name = name.to_owned();
        }
        if let Some(parent) = parent {
            node.parent = Some(parent);
        }
        self.touch(&id);
        true
    }

    /// Puts a node in or out of the trash.
    pub fn trash(&mut self, id: &str, trashed: bool) {
        let id = self.resolve(id).to_owned();
        if let Some(node) = self.nodes.get_mut(&id) {
            node.trashed = trashed;
            self.touch(&id);
        }
    }

    /// Deletes permanently; a folder takes what it holds.
    pub fn delete(&mut self, id: &str) {
        let id = self.resolve(id).to_owned();
        let held: Vec<String> = self
            .nodes
            .values()
            .filter(|n| n.parent.as_deref() == Some(id.as_str()) && !n.removed)
            .map(|n| n.id.clone())
            .collect();
        for child in held {
            self.delete(&child);
        }
        if let Some(node) = self.nodes.get_mut(&id) {
            node.removed = true;
            node.bytes = None;
            self.touch(&id);
        }
    }

    /// The nodes changed after `after`, oldest change first.
    pub fn changes_after(&self, after: u64) -> Vec<&Node> {
        let mut changed: Vec<&Node> = self.nodes.values().filter(|n| n.changed > after).collect();
        changed.sort_by_key(|n| n.changed);
        changed
    }

    /// Whether a page token is one the drive still honours.
    pub fn token_valid(&self, token: u64) -> bool {
        token >= self.oldest_valid && token <= self.seq
    }

    /// Every token handed out so far is dropped.
    pub fn expire_tokens(&mut self) {
        self.oldest_valid = self.seq + 1;
        // A change that moves nothing, so the next token is above the expired ones.
        self.seq += 1;
    }

    /// Starts a resumable upload.
    pub fn open_upload(&mut self, aim: Aim) -> String {
        let id = format!("upl{:04}", self.uploads.len() + 1);
        self.uploads.insert(
            id.clone(),
            Upload {
                aim,
                received: Vec::new(),
            },
        );
        id
    }

    /// An upload in flight.
    pub fn upload(&mut self, id: &str) -> Option<&mut Upload> {
        self.uploads.get_mut(id)
    }

    /// An upload that has finished.
    pub fn close_upload(&mut self, id: &str) -> Option<Upload> {
        self.uploads.remove(id)
    }
}

/// A node's `md5Checksum`.
pub fn checksum(node: &Node) -> Option<String> {
    node.bytes.as_deref().map(md5_hex)
}

impl Drive {
    /// Every live node but the root, the way a listing shows them (trashed ones are filtered by
    /// the query, as Drive does: a plain listing includes them).
    pub fn nodes_listed(&self) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| !n.removed && n.id != self.root)
            .collect()
    }
}
