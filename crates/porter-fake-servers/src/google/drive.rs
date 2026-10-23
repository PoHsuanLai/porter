//! The Drive behind the fake Google: files in two trees, the app data folder (the `appDataFolder`
//! alias names its root, in the `appDataFolder` space) and My Drive (the `root` alias, in the
//! `drive` space, which also holds files shared with the person that have no parent there), one
//! change counter that `changes.list` reads, and the resumable uploads in flight. My Drive can
//! hold Google's own documents and shortcuts, which have no bytes. Pure state; `drive_routes` is
//! the HTTP over it.
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
/// The id the app data folder's root answers to besides its own.
pub const ALIAS: &str = "appDataFolder";
/// The id My Drive's root answers to besides its own.
pub const MY_DRIVE: &str = "root";
/// The mime type of a shortcut.
pub const SHORTCUT: &str = "application/vnd.google-apps.shortcut";

/// Which space a file is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// The app data folder.
    AppData,
    /// My Drive and what is shared with the person.
    Drive,
}

impl Space {
    /// Its name in `spaces=`.
    pub fn name(self) -> &'static str {
        match self {
            Space::AppData => ALIAS,
            Space::Drive => "drive",
        }
    }
}

/// What a Google document or a shortcut is, in place of bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Native {
    /// Its mime type (`application/vnd.google-apps.document`, ...).
    pub mime: String,
    /// For a shortcut, the file it points at.
    pub target: Option<String>,
}
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
    /// The space it is in.
    pub space: Space,
    /// A Google document or a shortcut: no bytes, no folder.
    pub native: Option<Native>,
}

impl Node {
    fn new(id: &str, parent: Option<&str>, name: &str, space: Space) -> Self {
        Self {
            id: id.to_owned(),
            parent: parent.map(str::to_owned),
            name: name.to_owned(),
            bytes: None,
            version: 0,
            changed: 0,
            trashed: false,
            removed: false,
            space,
            native: None,
        }
    }

    /// Whether it is a folder.
    pub fn is_folder(&self) -> bool {
        self.bytes.is_none() && self.native.is_none()
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
    my_drive: String,
}

impl Default for Drive {
    fn default() -> Self {
        let root = "0AFakeAppDataRoot".to_owned();
        let my_drive = "0AFakeMyDriveRoot".to_owned();
        let mut nodes = BTreeMap::new();
        for (id, name, space) in [
            (&root, ALIAS, Space::AppData),
            (&my_drive, "My Drive", Space::Drive),
        ] {
            let mut node = Node::new(id, None, name, space);
            node.version = 1;
            nodes.insert(id.clone(), node);
        }
        Self {
            nodes,
            next: 1,
            seq: 0,
            oldest_valid: 0,
            limit: None,
            uploads: BTreeMap::new(),
            root,
            my_drive,
        }
    }
}

impl Drive {
    /// The id for an id or an alias.
    pub fn resolve<'a>(&'a self, id: &'a str) -> &'a str {
        match id {
            ALIAS => &self.root,
            MY_DRIVE => &self.my_drive,
            _ => id,
        }
    }

    /// Whether `id` is one of the two roots.
    pub fn is_root(&self, id: &str) -> bool {
        id == self.root || id == self.my_drive
    }

    /// Whether `id`'s parents lead to My Drive's root (removed nodes keep their parents).
    pub fn in_my_drive(&self, id: &str) -> bool {
        let mut at = self.nodes.get(id);
        while let Some(node) = at {
            if node.id == self.my_drive {
                return true;
            }
            at = node.parent.as_deref().and_then(|p| self.nodes.get(p));
        }
        false
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

    /// Every live node but the roots, in id order.
    pub fn live(&self) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.live() && !self.is_root(&n.id))
            .collect()
    }

    /// The live node at `names` below the app data folder's root.
    pub fn at(&self, names: &[String]) -> Option<&Node> {
        self.at_from(&self.root, names)
    }

    /// The live node at `names` below the folder `from` (an id or an alias).
    pub fn at_from(&self, from: &str, names: &[String]) -> Option<&Node> {
        names.iter().try_fold(self.get(from)?, |at, name| {
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

    /// The space of the folder `parent`, if it is a live folder.
    fn folder_space(&self, parent: &str) -> Option<Space> {
        self.get(parent).filter(|p| p.is_folder()).map(|p| p.space)
    }

    /// Makes a folder.
    pub fn make_folder(&mut self, parent: &str, name: &str) -> Result<String, Refused> {
        let parent = self.resolve(parent).to_owned();
        let space = self.folder_space(&parent).ok_or(Refused::NoParent)?;
        let id = self.fresh_id();
        self.nodes
            .insert(id.clone(), Node::new(&id, Some(&parent), name, space));
        self.touch(&id);
        Ok(id)
    }

    /// Plants a Google document of `mime` (or, with a `target`, a shortcut to it) in `parent`.
    pub fn plant_native(
        &mut self,
        parent: &str,
        name: &str,
        mime: &str,
        target: Option<&str>,
    ) -> Result<String, Refused> {
        let parent = self.resolve(parent).to_owned();
        let space = self.folder_space(&parent).ok_or(Refused::NoParent)?;
        let id = self.fresh_id();
        let mut node = Node::new(&id, Some(&parent), name, space);
        node.native = Some(Native {
            mime: mime.to_owned(),
            target: target.map(str::to_owned),
        });
        self.nodes.insert(id.clone(), node);
        self.touch(&id);
        Ok(id)
    }

    /// Plants a file someone shared with the person: in the `drive` space, with no parent the
    /// person can see.
    pub fn plant_shared(&mut self, name: &str, bytes: Vec<u8>) -> String {
        let id = self.fresh_id();
        let mut node = Node::new(&id, None, name, Space::Drive);
        node.bytes = Some(bytes);
        self.nodes.insert(id.clone(), node);
        self.touch(&id);
        id
    }

    /// The folders on `names` below the app data folder's root, made as needed.
    pub fn ensure(&mut self, names: &[String]) -> Result<String, Refused> {
        let root = self.root.clone();
        self.ensure_from(&root, names)
    }

    /// The folders on `names` below the folder `from`, made as needed.
    pub fn ensure_from(&mut self, from: &str, names: &[String]) -> Result<String, Refused> {
        let mut at = self.resolve(from).to_owned();
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
        let space = self.folder_space(&parent).ok_or(Refused::NoParent)?;
        let id = self.fresh_id();
        let mut node = Node::new(&id, Some(&parent), name, space);
        node.bytes = Some(bytes);
        self.nodes.insert(id.clone(), node);
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
    /// Every node but the roots in `space` that has not been permanently deleted, the way a
    /// listing shows them (trashed ones are filtered by the query, as Drive does: a plain listing
    /// includes them).
    pub fn nodes_listed(&self, space: Space) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| !n.removed && !self.is_root(&n.id) && n.space == space)
            .collect()
    }
}
