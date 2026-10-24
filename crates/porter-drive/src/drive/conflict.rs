//! What the drive did meanwhile, met with what was changed here. Nothing made here is lost.
//!
//! - **A newer version on the drive of a file with bytes not sent:** both are kept. The bytes
//!   written here become "Name (this computer).ext" ([`naming::THIS_COMPUTER`], numbered when
//!   that is taken) beside the drive's version, each names the other as its other version, and
//!   the copy is sent as a new file.
//! - **The drive removed a file with bytes not sent:** they are sent again as a new file where
//!   it is here.
//! - **Removed here, changed on the drive:** nothing is removed; the drive's version comes back.
//!   A folder whose only change on the drive is what this computer took out of it is removed
//!   still.
//! - **Renamed or moved here, changed on the drive:** when only its bytes changed there, the
//!   rename is made again on the new version; when the drive renamed or moved it too, the
//!   drive's name and place stand.
//! - **A name taken on the drive** by the time a new item is sent: a folder there is taken as
//!   the one made here (what was made in it goes into it), a file is sent as this computer's
//!   version beside it.

use super::{Inner, NodeId, held, now_secs};
use crate::index::{Awaiting, Base, Edit};
use crate::listing::Entry;
use crate::naming;
use porter_sync::{Conflict, RemoteId, RemoteSide};

impl Inner {
    /// Pairs `a` and `b` as two versions of one file.
    fn pair(&self, a: &NodeId, b: &NodeId) {
        let mut index = held(&self.index);
        index.conflicts.insert(a.clone(), b.clone());
        index.conflicts.insert(b.clone(), a.clone());
    }

    /// The other version of `id`, while it has one.
    pub(super) fn other(&self, id: &NodeId) -> Option<NodeId> {
        held(&self.index).conflicts.get(id).cloned()
    }

    /// `id` and the other version it named are no longer a pair.
    pub(super) fn unpair(&self, id: &NodeId) -> Option<NodeId> {
        let mut index = held(&self.index);
        let other = index.conflicts.remove(id)?;
        index.conflicts.remove(&other);
        Some(other)
    }

    /// A name for this computer's version of `name` in `folder` that nothing there has.
    fn this_computers(&self, folder: &NodeId, name: &str) -> String {
        let taken: Vec<String> = self
            .children(folder)
            .unwrap_or_default()
            .into_iter()
            .map(|node| node.name)
            .collect();
        naming::free(&naming::marked(name, naming::THIS_COMPUTER), |candidate| {
            taken.iter().any(|t| t == candidate)
        })
    }

    /// Takes the bytes not sent out of `edit` into a new file made here beside it, named
    /// `name`, queued to be sent; its node.
    fn split_off(&self, edit: &mut Edit, name: String) -> Option<NodeId> {
        let parent = edit.parent.clone();
        let dirty = edit.dirty.take()?;
        edit.session = None;
        let n = held(&self.index).take_local();
        let mut copy = Edit::made_here(n, parent, name, Some(dirty), now_secs());
        let id = copy.id.clone();
        {
            let mut index = held(&self.index);
            copy.seq = index.take_seq();
            index.edits.insert(id.clone(), copy);
        }
        self.ino(&id);
        Some(id)
    }

    /// Meets each change made here with the listing: see the module's rules. What changed is
    /// told.
    pub(super) fn meet_the_drive(&self) -> Vec<NodeId> {
        let sending = held(&self.sending).as_ref().map(|sent| sent.id.clone());
        let ids: Vec<RemoteId> = held(&self.index)
            .edits
            .keys()
            .filter_map(|id| match id {
                NodeId::Remote(item) => Some(item.clone()),
                _ => None,
            })
            .collect();
        let mut changed = Vec::new();
        for item in ids {
            let id = NodeId::Remote(item.clone());
            if sending.as_ref() == Some(&id) {
                continue;
            }
            let Ok(listed) = self.listing.entry(&item) else {
                continue;
            };
            let Some(edit) = self.edit(&id) else {
                continue;
            };
            let Some(base) = edit.base.clone() else {
                continue;
            };
            let behind = |version: Option<&str>| {
                edit.awaiting
                    .as_ref()
                    .is_some_and(|awaiting| awaiting.before.as_deref() == version)
            };
            match listed {
                None if edit.removed || behind(None) => {}
                None => changed.extend(self.gone_from_the_drive(edit)),
                Some(entry)
                    if entry.version.0 == base.version || behind(Some(&entry.version.0)) => {}
                Some(entry) => changed.extend(self.newer_on_the_drive(edit, &base, entry)),
            }
        }
        if !changed.is_empty() {
            self.touched();
            self.wake.notify_one();
        }
        changed
    }

    /// The drive no longer has `edit`'s item.
    fn gone_from_the_drive(&self, mut edit: Edit) -> Vec<NodeId> {
        let id = edit.id.clone();
        let mut changed = vec![id.clone(), edit.parent.clone()];
        held(&self.index).edits.remove(&id);
        let name = edit.name.clone();
        if !edit.folder
            && let Some(copy) = self.split_off(&mut edit, name)
        {
            // Sent again as a new file where it is here, under the same inode number.
            let ino = self.ino(&id);
            held(&self.index).inos.insert(super::key_text(&copy), ino);
            held(&self.inodes).insert(ino, copy.clone());
            self.rekey(&id, &copy);
            if let Some(other) = self.unpair(&id) {
                self.pair(&copy, &other);
            }
            changed.push(copy);
        } else {
            self.unpair(&id);
        }
        changed
    }

    /// The drive has a newer version of `edit`'s item than `base`, the one it was changed from.
    fn newer_on_the_drive(&self, mut edit: Edit, base: &Base, entry: Entry) -> Vec<NodeId> {
        let id = edit.id.clone();
        let mut changed = vec![id.clone(), edit.parent.clone()];
        let rebased = Base {
            version: entry.version.0.clone(),
            parent: entry.parent.clone().map(|p| p.0),
            name: entry.name.clone(),
            size: entry.size.0,
            modified: entry.modified.map(|m| m.0),
        };
        let moved_there = entry.name != base.name
            || entry.parent.as_ref().map(|p| p.0.as_str()) != base.parent.as_deref();
        if edit.removed {
            let refilled = edit.folder
                && self
                    .listing
                    .children(Some(&entry.id))
                    .unwrap_or_default()
                    .iter()
                    .any(|child| {
                        self.edit(&NodeId::Remote(child.id.clone()))
                            .is_none_or(|e| !e.removed && e.parent == id)
                    });
            match edit.folder && !refilled && !moved_there {
                // Emptied by what this computer took out of it: removed still.
                true => {
                    edit.base = Some(rebased);
                    held(&self.index).edits.insert(id, edit);
                }
                false => {
                    held(&self.index).edits.remove(&id);
                }
            }
            return changed;
        }
        if !edit.folder && edit.dirty.is_some() {
            let name = self.this_computers(&edit.parent, &edit.name);
            if let Some(copy) = self.split_off(&mut edit, name) {
                self.unpair(&id);
                self.pair(&copy, &id);
                changed.push(copy);
            }
        }
        if moved_there {
            // Renamed or moved on the drive too: its name and place stand.
            edit.parent = entry.parent.clone().map_or(NodeId::Top, NodeId::Remote);
            edit.name = entry.name.clone();
        }
        edit.base = Some(rebased);
        edit.awaiting = None;
        let mut index = held(&self.index);
        match edit.pending() {
            true => index.edits.insert(id, edit),
            false => index.edits.remove(&id),
        };
        changed
    }

    /// The drive refused `id`'s step: it has another version than the one it was made against,
    /// or the name is taken there. Whether it may be tried again at once.
    pub(super) fn met_conflict(&self, id: &NodeId, conflict: &Conflict) -> bool {
        let RemoteSide::Exists(there, version) = &conflict.remote else {
            return false;
        };
        let Some(mut edit) = self.edit(id) else {
            return false;
        };
        let NodeId::Local(n) = id else {
            return false;
        };
        if edit.base.is_some() {
            return false;
        }
        if edit.folder {
            // A folder of that name is there: what was made in this one goes into it.
            let to = NodeId::Remote(there.clone());
            if held(&self.index).edits.contains_key(&to) {
                return false;
            }
            held(&self.index).edits.remove(id);
            edit.id = to.clone();
            edit.base = Some(Base {
                version: version.0.clone(),
                parent: match &edit.parent {
                    NodeId::Remote(parent) => Some(parent.0.clone()),
                    _ => None,
                },
                name: edit.name.clone(),
                size: 0,
                modified: None,
            });
            edit.awaiting = Some(Awaiting {
                before: None,
                passes: 0,
            });
            held(&self.index).edits.insert(to.clone(), edit);
            self.alias(*n, &to);
            return true;
        }
        let name = self.this_computers(&edit.parent, &edit.name);
        edit.name = name;
        held(&self.index).edits.insert(id.clone(), edit);
        self.pair(id, &NodeId::Remote(there.clone()));
        self.touched();
        true
    }
}
