//! The tree as shown: the listing with what was changed here and not on the drive yet laid over
//! it. An item moved, renamed or removed here is shown where it is here; one made here is shown
//! by its number until the drive gives it an id, and then by that id under the same inode number
//! and path.

use super::{Attrs, DEPTH, DriveError, Inner, Node, NodeId, NodeKind, held, ino_for, key_text};
use crate::index::{Edit, Index};
use crate::listing::Entry;
use crate::local_only;
use crate::naming;
use crate::online;
use porter_ids::{Bytes, UnixSeconds};
use porter_sync::{ItemKind, RemoteId, RemoteVersion};

/// `listed` (the listing's entry of `id`, if it has one) as `edit` changed it here; `None` when
/// it was removed here.
pub(super) fn overlay(listed: Option<Entry>, id: &RemoteId, edit: &Edit) -> Option<Entry> {
    if edit.removed {
        return None;
    }
    let base = edit.base.as_ref()?;
    let mut entry = listed.unwrap_or_else(|| Entry {
        id: id.clone(),
        parent: base.parent.clone().map(RemoteId),
        name: base.name.clone(),
        kind: match edit.folder {
            true => ItemKind::Folder,
            false => ItemKind::File,
        },
        size: Bytes(base.size),
        version: RemoteVersion(base.version.clone()),
        hash: None,
        modified: base.modified.map(UnixSeconds),
    });
    if entry.version.0 != base.version {
        // What was sent from here, which the listing has not caught up with.
        entry.version = RemoteVersion(base.version.clone());
        entry.size = Bytes(base.size);
        entry.modified = base.modified.map(UnixSeconds);
        entry.hash = None;
    }
    entry.name = edit.name.clone();
    match &edit.parent {
        NodeId::Top => entry.parent = None,
        NodeId::Remote(parent) => entry.parent = Some(parent.clone()),
        NodeId::Local(_) => {}
    }
    if let Some(dirty) = &edit.dirty {
        entry.size = Bytes(dirty.size);
        entry.modified = Some(UnixSeconds(dirty.modified));
    }
    Some(entry)
}

impl Inner {
    /// The node `id` names now: following what an item made here became.
    pub(super) fn canonical(&self, id: &NodeId) -> NodeId {
        Self::canonical_in(&held(&self.index), id)
    }

    pub(super) fn canonical_in(index: &Index, id: &NodeId) -> NodeId {
        let mut at = id.clone();
        for _ in 0..DEPTH {
            match &at {
                NodeId::Local(n) => match index.aliases.get(n) {
                    Some(next) => at = next.clone(),
                    None => break,
                },
                _ => break,
            }
        }
        at
    }

    /// The change made here of `id`, if any.
    pub(super) fn edit(&self, id: &NodeId) -> Option<Edit> {
        held(&self.index).edits.get(id).cloned()
    }

    /// The drive's item `id` as it is here: the listing's entry with what was changed here.
    pub(super) fn entry(&self, id: &RemoteId) -> Result<Option<Entry>, DriveError> {
        let listed = self.listing.entry(id)?;
        let edit = self.edit(&NodeId::Remote(id.clone()));
        Ok(match edit {
            None => listed,
            Some(edit) => overlay(listed, id, &edit),
        })
    }

    /// The entry of `id` as [`Inner::entry`] shows it, or, removed here while it is open, the
    /// drive's: it stays usable until it is closed.
    pub(super) fn entry_open(&self, id: &RemoteId) -> Result<Option<Entry>, DriveError> {
        let node = NodeId::Remote(id.clone());
        match self.edit(&node).is_some_and(|edit| edit.removed) && self.is_open(&node) {
            true => Ok(self.listing.entry(id)?),
            false => self.entry(id),
        }
    }

    pub(super) fn top(&self) -> Node {
        Node {
            id: NodeId::Top,
            parent: NodeId::Top,
            name: String::new(),
            kind: NodeKind::Folder,
            size: 0,
            modified: None,
        }
    }

    pub(super) fn attrs_of_entry(entry: &Entry) -> Attrs {
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

    fn attrs_of_made(edit: &Edit) -> Attrs {
        Attrs {
            kind: match edit.folder {
                true => NodeKind::Folder,
                false => NodeKind::File,
            },
            size: edit.dirty.as_ref().map_or(0, |dirty| dirty.size),
            modified: Some(UnixSeconds(
                edit.dirty
                    .as_ref()
                    .map_or(edit.made, |dirty| dirty.modified),
            )),
        }
    }

    pub(super) fn attrs(&self, id: &NodeId) -> Result<Option<Attrs>, DriveError> {
        Ok(match self.canonical(id) {
            NodeId::Top => Some(Attrs {
                kind: NodeKind::Folder,
                size: 0,
                modified: None,
            }),
            NodeId::Remote(id) => self.entry(&id)?.as_ref().map(Self::attrs_of_entry),
            local @ NodeId::Local(_) => self.edit(&local).as_ref().map(Self::attrs_of_made),
        })
    }

    /// Whether `id` is a folder as shown.
    pub(super) fn is_folder(&self, id: &NodeId) -> Result<bool, DriveError> {
        Ok(matches!(
            self.attrs(id)?,
            Some(Attrs {
                kind: NodeKind::Folder,
                ..
            })
        ))
    }

    pub(super) fn children(&self, folder: &NodeId) -> Result<Vec<Node>, DriveError> {
        let folder = self.canonical(folder);
        let listed = match &folder {
            NodeId::Top => self.listing.children(None)?,
            NodeId::Remote(id) => match self.entry(id)? {
                Some(entry) if entry.kind == ItemKind::Folder => self.listing.children(Some(id))?,
                _ => return Ok(Vec::new()),
            },
            NodeId::Local(_) => match self.edit(&folder) {
                Some(edit) if edit.folder => Vec::new(),
                _ => return Ok(Vec::new()),
            },
        };
        let (mut remote, placed): (Vec<Entry>, Vec<Edit>) = {
            let index = held(&self.index);
            let remote = listed
                .into_iter()
                .filter(|entry| !index.edits.contains_key(&NodeId::Remote(entry.id.clone())))
                .collect();
            let placed = index
                .edits
                .values()
                .filter(|edit| !edit.removed && edit.parent == folder)
                .cloned()
                .collect();
            (remote, placed)
        };
        let mut made = Vec::new();
        for edit in placed {
            match &edit.id {
                NodeId::Remote(id) => {
                    if let Some(entry) = overlay(self.listing.entry(id)?, id, &edit) {
                        remote.push(entry);
                    }
                }
                NodeId::Local(_) => made.push(edit),
                NodeId::Top => {}
            }
        }
        remote.sort_by(|a, b| a.id.cmp(&b.id));
        made.sort_by(|a, b| a.id.cmp(&b.id));
        let bases: Vec<String> = remote
            .iter()
            .map(|entry| naming::base_of(&entry.name, &entry.kind))
            .chain(made.iter().map(|edit| naming::shown_base(&edit.name)))
            .collect();
        let names = naming::assign(&bases);
        let mut nodes = Vec::with_capacity(names.len());
        let mut names = names.into_iter();
        for entry in &remote {
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
        for edit in &made {
            let attrs = Self::attrs_of_made(edit);
            nodes.push(Node {
                id: edit.id.clone(),
                parent: folder.clone(),
                name: names.next().unwrap_or_default(),
                kind: attrs.kind,
                size: attrs.size,
                modified: attrs.modified,
            });
        }
        for node in &nodes {
            self.ino(&node.id);
        }
        Ok(nodes)
    }

    pub(super) fn parent_of(&self, id: &NodeId) -> Result<Option<NodeId>, DriveError> {
        let id = self.canonical(id);
        if let Some(edit) = self.edit(&id) {
            return Ok((!edit.removed).then_some(edit.parent));
        }
        Ok(match &id {
            NodeId::Top => Some(NodeId::Top),
            NodeId::Remote(id) => self
                .listing
                .entry(id)?
                .map(|entry| entry.parent.map_or(NodeId::Top, NodeId::Remote)),
            NodeId::Local(_) => None,
        })
    }

    pub(super) fn node(&self, id: &NodeId) -> Result<Option<Node>, DriveError> {
        let id = self.canonical(id);
        if id == NodeId::Top {
            return Ok(Some(self.top()));
        }
        let Some(parent) = self.parent_of(&id)? else {
            return Ok(None);
        };
        Ok(self
            .children(&parent)?
            .into_iter()
            .find(|node| node.id == id))
    }

    /// The inode number of `id`: its own, or that of what it was made as here.
    pub(super) fn ino(&self, id: &NodeId) -> u64 {
        let (id, kept) = {
            let index = held(&self.index);
            let id = Self::canonical_in(&index, id);
            let kept = index.inos.get(&key_text(&id)).copied();
            (id, kept)
        };
        let ino = kept.unwrap_or_else(|| ino_for(&id));
        held(&self.inodes).insert(ino, id);
        ino
    }

    /// Whether `id` stays on this computer and is never sent: a lock file, or anything in the
    /// desktop's trash at the top of the drive (see [`crate::local_only`]).
    pub(super) fn local_only(&self, id: &NodeId) -> bool {
        let mut at = self.canonical(id);
        let mut first = true;
        for _ in 0..DEPTH {
            if at == NodeId::Top {
                return false;
            }
            let name = self.node_name(&at);
            let Ok(Some(parent)) = self.parent_of(&at) else {
                return false;
            };
            if let Some(name) = name {
                if first && local_only::is_lock_file(&name) && !self.is_folder(&at).unwrap_or(true)
                {
                    return true;
                }
                if parent == NodeId::Top && local_only::is_trash(&name) {
                    return true;
                }
            }
            first = false;
            at = parent;
        }
        false
    }

    /// The name `id` has here (its drive name as changed here, not as shown).
    pub(super) fn node_name(&self, id: &NodeId) -> Option<String> {
        if let Some(edit) = self.edit(id) {
            return Some(edit.name);
        }
        match id {
            NodeId::Remote(id) => self.listing.entry(id).ok().flatten().map(|e| e.name),
            _ => None,
        }
    }
}
