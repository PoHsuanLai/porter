//! What the drive did meanwhile, met with what was changed here. Nothing made here is lost.
//!
//! - **A newer version on the drive of a file with bytes not sent:** both are kept. The bytes
//!   written here become "Name (this computer).ext" ([`naming::THIS_COMPUTER`], numbered when
//!   that is taken among the drive's names there) beside the drive's version, each names the
//!   other as its other version, and the copy is sent as a new file. What has the file open to
//!   write goes on writing this computer's version, under the inode number it opened.
//! - **The drive removed a file with bytes not sent:** they are sent again as a new file where
//!   it is here, under the same inode number.
//! - **Removed here, changed on the drive:** nothing is removed; the drive's version comes back.
//!   A folder whose only change on the drive is what this computer took out of it is removed
//!   still.
//! - **Renamed or moved here, changed on the drive:** when only its bytes changed there, the
//!   rename is made again on the new version; when the drive renamed or moved it too, the
//!   drive's name and place stand.
//! - **A name taken on the drive** by the time a new item is sent: a folder there is taken as
//!   the one made here (what was made in it goes into it), a file is sent as this computer's
//!   version beside it.
//!
//! Each change is met under the lock of its bytes and the index's, on the change as it is then.

use super::{Inner, NodeId, held, ino_for, key_text, now_secs};
use crate::index::{Awaiting, Base, Dirty, Edit, Index};
use crate::listing::Entry;
use crate::naming;
use porter_sync::{Conflict, RemoteId, RemoteSide};

/// How many times a change is looked at again when it changed while it was met.
const TRIES: usize = 8;

/// What the drive did to an item changed here.
enum Step {
    /// It no longer has it.
    Gone,
    /// It has a newer version than the change was made against.
    Newer(Entry),
}

/// What meeting one change did: the nodes whose details changed, and the nodes whose writers
/// (and inode number) went to another (`true`: the one left behind is given a new number).
#[derive(Default)]
struct Met {
    changed: Vec<NodeId>,
    moved: Vec<(NodeId, NodeId, bool)>,
}

impl Inner {
    /// The other version of `id`, while it has one.
    pub(super) fn other(&self, id: &NodeId) -> Option<NodeId> {
        held(&self.index).conflicts.get(id).cloned()
    }

    /// A name for this computer's version of `name` in `folder` that none of the drive's names
    /// there (as changed here) has.
    fn this_computers(&self, folder: &NodeId, name: &str) -> String {
        let taken: Vec<String> = self
            .children(folder)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|node| self.node_name(&node.id))
            .collect();
        naming::free(&naming::marked(name, naming::THIS_COMPUTER), |candidate| {
            taken.iter().any(|t| t == candidate)
        })
    }

    /// Takes the bytes not sent out of `edit` into a new file made here beside it, named
    /// `name`, queued to be sent (a new generation of them); its node.
    fn split_off(&self, index: &mut Index, edit: &mut Edit, name: String) -> Option<NodeId> {
        let dirty = edit.dirty.take()?;
        // Its session sent bytes to the drive's item, which this copy is not.
        self.abandon(edit);
        let n = index.take_local();
        let dirty = Dirty {
            generation: index.take_generation(),
            ..dirty
        };
        let mut copy = Edit::made_here(n, edit.parent.clone(), name, Some(dirty), now_secs());
        copy.seq = index.take_seq();
        let id = copy.id.clone();
        index.edits.insert(id.clone(), copy);
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
            changed.extend(self.meet(&id, listed));
        }
        if !changed.is_empty() {
            self.touched();
            self.wake.notify_one();
        }
        changed
    }

    /// Meets the change of `id` with the drive's entry of it, `listed`.
    fn meet(&self, id: &NodeId, listed: Option<Entry>) -> Vec<NodeId> {
        for _ in 0..TRIES {
            let Some(edit) = self.edit(id) else {
                return Vec::new();
            };
            let Some(base) = edit.base.clone() else {
                return Vec::new();
            };
            let behind = |version: Option<&str>| {
                edit.awaiting
                    .as_ref()
                    .is_some_and(|awaiting| awaiting.before.as_deref() == version)
            };
            let step = match &listed {
                None if edit.removed || behind(None) => return Vec::new(),
                None => Step::Gone,
                Some(entry)
                    if entry.version.0 == base.version || behind(Some(&entry.version.0)) =>
                {
                    return Vec::new();
                }
                Some(entry) => Step::Newer(entry.clone()),
            };
            // This computer's version's name, chosen before the index is locked (choosing reads
            // the folder).
            let name = match (&step, edit.folder || edit.removed || edit.dirty.is_none()) {
                (_, true) => None,
                (Step::Gone, false) => Some(edit.name.clone()),
                (Step::Newer(_), false) => Some(self.this_computers(&edit.parent, &edit.name)),
            };
            let lock = edit.dirty.as_ref().map(|dirty| self.file_lock(dirty.file));
            let bytes = lock.as_ref().map(|lock| held(lock));
            let met = {
                let mut index = held(&self.index);
                if index.edits.get(id) != Some(&edit) {
                    // Changed meanwhile: met again as it is now.
                    continue;
                }
                index.edits.remove(id);
                let met = match step {
                    Step::Gone => self.gone_from_the_drive(&mut index, edit, name),
                    Step::Newer(entry) => {
                        self.newer_on_the_drive(&mut index, edit, &base, entry, name)
                    }
                };
                // Its writers follow the bytes before anything can write again; who reads it
                // still reads (and closes) what it opened.
                for (from, to, _) in &met.moved {
                    self.rekey_writes(from, to);
                }
                met
            };
            for (from, to, fresh) in &met.moved {
                self.hand_over(from, to, *fresh);
            }
            drop(bytes);
            return met.changed;
        }
        Vec::new()
    }

    /// `from`'s inode number goes to `to`, which its writers went to (`fresh`: `from` stays,
    /// under a new number).
    fn hand_over(&self, from: &NodeId, to: &NodeId, fresh: bool) {
        let ino = self.ino(from);
        let renumbered = fresh.then(|| ino_for(&NodeId::Local(held(&self.index).take_local())));
        {
            let mut index = held(&self.index);
            index.inos.insert(key_text(to), ino);
            if let Some(new) = renumbered {
                index.inos.insert(key_text(from), new);
            }
        }
        {
            let mut inodes = held(&self.inodes);
            inodes.insert(ino, to.clone());
            if let Some(new) = renumbered {
                inodes.insert(new, from.clone());
            }
        }
        self.touched();
    }

    /// The drive no longer has `edit`'s item.
    fn gone_from_the_drive(&self, index: &mut Index, mut edit: Edit, name: Option<String>) -> Met {
        let id = edit.id.clone();
        let mut met = Met {
            changed: vec![id.clone(), edit.parent.clone()],
            ..Met::default()
        };
        let copy = name.and_then(|name| self.split_off(index, &mut edit, name));
        match copy {
            Some(copy) => {
                // Sent again as a new file where it is here, under the same inode number.
                if let Some(other) = index.unpair(&id) {
                    index.pair(&copy, &other);
                }
                met.moved.push((id, copy.clone(), false));
                met.changed.push(copy);
            }
            None => {
                index.unpair(&id);
            }
        }
        met
    }

    /// The drive has a newer version of `edit`'s item than `base`, the one it was changed from.
    fn newer_on_the_drive(
        &self,
        index: &mut Index,
        mut edit: Edit,
        base: &Base,
        entry: Entry,
        name: Option<String>,
    ) -> Met {
        let id = edit.id.clone();
        let mut met = Met {
            changed: vec![id.clone(), edit.parent.clone()],
            ..Met::default()
        };
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
                        index
                            .edits
                            .get(&NodeId::Remote(child.id.clone()))
                            .is_none_or(|e| !e.removed && e.parent == id)
                    });
            if edit.folder && !refilled && !moved_there {
                // Emptied by what this computer took out of it: removed still.
                edit.base = Some(rebased);
                index.edits.insert(id, edit);
            }
            return met;
        }
        if let Some(name) = name
            && let Some(copy) = self.split_off(index, &mut edit, name)
        {
            index.unpair(&id);
            index.pair(&copy, &id);
            // What has it open to write goes on writing this computer's version.
            if self.written_by_handle(&id) {
                met.moved.push((id.clone(), copy.clone(), true));
            }
            met.changed.push(copy);
        }
        if moved_there {
            // Renamed or moved on the drive too: its name and place stand.
            edit.parent = entry.parent.clone().map_or(NodeId::Top, NodeId::Remote);
            edit.name = entry.name.clone();
        }
        edit.base = Some(rebased);
        edit.awaiting = None;
        if edit.pending() {
            index.edits.insert(id, edit);
        }
        met
    }

    /// The drive refused `id`'s step: it has another version than the one it was made against,
    /// or the name is taken there. Whether it may be tried again at once.
    pub(super) fn met_conflict(&self, id: &NodeId, conflict: &Conflict) -> bool {
        let RemoteSide::Exists(there, version) = &conflict.remote else {
            return false;
        };
        let NodeId::Local(n) = id else {
            return false;
        };
        let Some(edit) = self.edit(id) else {
            return false;
        };
        if edit.base.is_some() {
            return false;
        }
        if edit.folder {
            // A folder of that name is there: what was made in this one goes into it.
            let to = NodeId::Remote(there.clone());
            let adopted = {
                let mut index = held(&self.index);
                let made = match index.edits.contains_key(&to) {
                    true => None,
                    false => index.edits.remove(id),
                };
                match made {
                    Some(mut edit) => {
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
                        index.edits.insert(to.clone(), edit);
                        true
                    }
                    None => false,
                }
            };
            if adopted {
                self.alias(*n, &to);
            }
            return adopted;
        }
        let name = self.this_computers(&edit.parent, &edit.name);
        let mut index = held(&self.index);
        let Some(edit) = index.edits.get_mut(id) else {
            return false;
        };
        edit.name = name;
        index.pair(id, &NodeId::Remote(there.clone()));
        drop(index);
        self.touched();
        true
    }
}
