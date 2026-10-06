//! The drive behind the fake Graph: items in a tree (a root, the `Apps/Quire` folder that is the
//! app folder, whatever the app made below it), a change counter that delta queries read, and the
//! upload sessions in flight. Pure state; `routes` is the HTTP over it.
//!
//! What it keeps from OneDrive: an item's `eTag` is `"{ID},version"` with its quotes and changes
//! with every write; a parent folder changes when a child does; a deleted folder is reported once,
//! its children not at all; a re-created path is a new item with a new id.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The id of the drive's root.
pub const ROOT: &str = "ROOT";
/// What a non-final upload chunk's size must be a multiple of (320 KiB).
pub const CHUNK_UNIT: usize = 327_680;

/// What an item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// A folder.
    Folder,
    /// A file and its bytes.
    File(Vec<u8>),
}

/// One item, live or deleted.
#[derive(Debug, Clone)]
pub struct Node {
    /// Its id.
    pub id: String,
    /// Its parent's id; the root has none.
    pub parent: Option<String>,
    /// Its name.
    pub name: String,
    /// Folder or file.
    pub body: Body,
    /// Counts its changes; the `eTag` is made from it.
    pub version: u64,
    /// The change counter at its last change (what delta orders by).
    pub changed: u64,
    /// Whether it was deleted.
    pub deleted: bool,
}

impl Node {
    /// The `eTag`, quotes included.
    pub fn etag(&self) -> String {
        format!("\"{{{}}},{}\"", self.id, self.version)
    }

    /// The size: a file's bytes, zero for a folder.
    pub fn size(&self) -> u64 {
        match &self.body {
            Body::File(bytes) => bytes.len() as u64,
            Body::Folder => 0,
        }
    }

    /// A stand-in for the QuickXorHash: base64 of 20 bytes made from the content.
    pub fn quick_xor(&self) -> Option<String> {
        use base64::Engine;
        match &self.body {
            Body::File(bytes) => {
                Some(base64::engine::general_purpose::STANDARD.encode(&Sha256::digest(bytes)[..20]))
            }
            Body::Folder => None,
        }
    }
}

/// Why the drive refused a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The limit would be passed (`507`).
    Full,
    /// A name on the path is a file, or a folder is written as a file (`409`).
    Blocked,
}

/// Where an upload session will put its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aim {
    /// An item that exists.
    Item(String),
    /// A new item: the folder names below the app folder, then the file's name.
    New(Vec<String>, String),
}

/// An upload session in flight.
#[derive(Debug, Clone)]
pub struct Upload {
    /// Where it ends.
    pub aim: Aim,
    /// Whether an existing item at the end is a conflict.
    pub fail_if_exists: bool,
    /// The `If-Match` it was opened with.
    pub if_match: Option<String>,
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
    approot: String,
}

impl Default for Drive {
    fn default() -> Self {
        let mut drive = Self {
            nodes: BTreeMap::new(),
            next: 1,
            seq: 0,
            oldest_valid: 0,
            limit: None,
            uploads: BTreeMap::new(),
            approot: String::new(),
        };
        drive.nodes.insert(
            ROOT.to_owned(),
            Node {
                id: ROOT.to_owned(),
                parent: None,
                name: String::new(),
                body: Body::Folder,
                version: 1,
                changed: 0,
                deleted: false,
            },
        );
        let apps = drive.folder(ROOT, "Apps");
        drive.approot = drive.folder(&apps, "Quire");
        drive.seq = 0;
        for node in drive.nodes.values_mut() {
            node.changed = 0;
        }
        drive
    }
}

impl Drive {
    /// The app folder's id.
    pub fn approot(&self) -> &str {
        &self.approot
    }

    /// The change counter now.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Delta tokens older than the counter now are no longer valid.
    pub fn expire_tokens(&mut self) {
        // The counter moves on, so even a token read a moment ago is older than the server's.
        self.seq += 1;
        self.oldest_valid = self.seq;
    }

    /// Whether a token read at `from` still resumes.
    pub fn resumes(&self, from: u64) -> bool {
        from >= self.oldest_valid && from <= self.seq
    }

    /// Gives the drive room for `total` bytes.
    pub fn set_limit(&mut self, total: u64) {
        self.limit = Some(total);
    }

    /// The limit, if one was set.
    pub fn limit(&self) -> Option<u64> {
        self.limit
    }

    /// Bytes in live files.
    pub fn used(&self) -> u64 {
        self.nodes
            .values()
            .filter(|n| !n.deleted)
            .map(Node::size)
            .sum()
    }

    /// A live item.
    pub fn live(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id).filter(|n| !n.deleted)
    }

    /// An item, deleted or not.
    pub fn any(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// The live child of `parent` named `name`.
    pub fn child(&self, parent: &str, name: &str) -> Option<&Node> {
        self.nodes
            .values()
            .find(|n| !n.deleted && n.parent.as_deref() == Some(parent) && n.name == name)
    }

    /// The live item at `names` below the app folder; no names is the app folder.
    pub fn at(&self, names: &[String]) -> Option<&Node> {
        names
            .iter()
            .try_fold(self.live(&self.approot)?, |at, name| {
                self.child(&at.id, name)
            })
    }

    /// Whether `id` is `scope` or below it (deleted items keep their parents).
    pub fn within(&self, id: &str, scope: &str) -> bool {
        let mut at = Some(id);
        while let Some(current) = at {
            if current == scope {
                return true;
            }
            at = self.nodes.get(current).and_then(|n| n.parent.as_deref());
        }
        false
    }

    /// Items below `scope` (itself included) changed after `from` up to `end`, oldest change
    /// first. A listing from the start (`from == 0`) holds live items only.
    pub fn changes(&self, scope: &str, from: u64, end: u64) -> Vec<&Node> {
        let mut found: Vec<&Node> = self
            .nodes
            .values()
            .filter(|n| n.changed > from && n.changed <= end)
            .filter(|n| from > 0 || !n.deleted)
            .filter(|n| self.within(&n.id, scope))
            .collect();
        found.sort_by(|a, b| a.changed.cmp(&b.changed).then_with(|| a.id.cmp(&b.id)));
        found
    }

    fn touch(&mut self, id: &str) {
        self.seq += 1;
        let seq = self.seq;
        let mut at = Some(id.to_owned());
        while let Some(current) = at {
            let Some(node) = self.nodes.get_mut(&current) else {
                break;
            };
            node.changed = seq;
            at = node.parent.clone();
        }
    }

    fn fresh(&mut self) -> String {
        let id = format!("ITEM{}", self.next);
        self.next += 1;
        id
    }

    /// Makes a folder.
    fn folder(&mut self, parent: &str, name: &str) -> String {
        let id = self.fresh();
        self.nodes.insert(
            id.clone(),
            Node {
                id: id.clone(),
                parent: Some(parent.to_owned()),
                name: name.to_owned(),
                body: Body::Folder,
                version: 1,
                changed: 0,
                deleted: false,
            },
        );
        self.touch(&id);
        id
    }

    /// The folder at `names` below `from`, made as far as it is missing.
    pub fn ensure(&mut self, from: &str, names: &[String]) -> Result<String, Refused> {
        names.iter().try_fold(from.to_owned(), |at, name| {
            match self
                .child(&at, name)
                .map(|n| (n.id.clone(), n.body.clone()))
            {
                Some((id, Body::Folder)) => Ok(id),
                Some((_, Body::File(_))) => Err(Refused::Blocked),
                None => Ok(self.folder(&at, name)),
            }
        })
    }

    /// Makes the folder `name` in `parent`; `None` if the name is taken.
    pub fn mkdir(&mut self, parent: &str, name: &str) -> Option<String> {
        self.child(parent, name)
            .is_none()
            .then(|| self.folder(parent, name))
    }

    /// Writes `bytes` as the file `name` in `parent`, replacing a file there.
    pub fn write(&mut self, parent: &str, name: &str, bytes: Vec<u8>) -> Result<String, Refused> {
        let existing = self.child(parent, name).map(|n| (n.id.clone(), n.size()));
        let old = existing.as_ref().map_or(0, |(_, size)| *size);
        if self
            .limit
            .is_some_and(|limit| self.used() - old + bytes.len() as u64 > limit)
        {
            return Err(Refused::Full);
        }
        match existing {
            Some((id, _)) => {
                let node = self.nodes.get_mut(&id).ok_or(Refused::Blocked)?;
                if node.body == Body::Folder {
                    return Err(Refused::Blocked);
                }
                node.body = Body::File(bytes);
                node.version += 1;
                self.touch(&id);
                Ok(id)
            }
            None => {
                let id = self.fresh();
                self.nodes.insert(
                    id.clone(),
                    Node {
                        id: id.clone(),
                        parent: Some(parent.to_owned()),
                        name: name.to_owned(),
                        body: Body::File(bytes),
                        version: 1,
                        changed: 0,
                        deleted: false,
                    },
                );
                self.touch(&id);
                Ok(id)
            }
        }
    }

    /// Replaces the bytes of the file `id`.
    pub fn replace(&mut self, id: &str, bytes: Vec<u8>) -> Result<(), Refused> {
        let (parent, name) = self
            .live(id)
            .and_then(|n| Some((n.parent.clone()?, n.name.clone())))
            .ok_or(Refused::Blocked)?;
        self.write(&parent, &name, bytes).map(|_| ())
    }

    /// Deletes an item and everything below it; only the item itself is reported by delta.
    pub fn delete(&mut self, id: &str) {
        let below: Vec<String> = self
            .nodes
            .keys()
            .filter(|other| other.as_str() != id && self.within(other, id))
            .cloned()
            .collect();
        for other in below {
            if let Some(node) = self.nodes.get_mut(&other) {
                node.deleted = true;
            }
        }
        if let Some(node) = self.nodes.get_mut(id) {
            node.deleted = true;
            node.version += 1;
        }
        self.touch(id);
    }

    /// Opens an upload session.
    pub fn open_upload(&mut self, upload: Upload) -> String {
        let id = format!("SESSION{}", self.next);
        self.next += 1;
        self.uploads.insert(id.clone(), upload);
        id
    }

    /// An upload session in flight.
    pub fn upload_mut(&mut self, id: &str) -> Option<&mut Upload> {
        self.uploads.get_mut(id)
    }

    /// Ends an upload session.
    pub fn close_upload(&mut self, id: &str) -> Option<Upload> {
        self.uploads.remove(id)
    }
}
