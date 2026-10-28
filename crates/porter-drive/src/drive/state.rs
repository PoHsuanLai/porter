//! Where each item stands, as Files shows it: a file's own state, a folder's from what is under
//! it.

use super::{Drive, Inner, Node, NodeId, NodeKind, held};
use crate::Reason;
use crate::listing::Entry;

/// Where an item stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ItemState {
    /// Listed, not here: it downloads when opened.
    Cloud,
    /// Downloading; the fraction says how far.
    Downloading,
    /// Here, and the same as the drive's version.
    Downloaded,
    /// Made or changed here and not sent yet ("Waiting to upload"): while offline, paused, or
    /// behind other changes.
    Waiting,
    /// Being sent; the fraction says how far.
    Uploading,
    /// Two versions of it are kept side by side (the drive's, and this computer's); each names
    /// the other.
    Conflict,
    /// It could not be downloaded or sent; the reason says why.
    Error,
}

/// An item's details.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Where it stands.
    pub state: ItemState,
    /// How far a download or upload is, 0 to 1, while it moves.
    pub fraction: Option<f64>,
    /// What went wrong, when the state is an error, or why it waits.
    pub reason: Option<Reason>,
    /// Whether it is kept downloaded, itself or with a folder above it.
    pub kept: bool,
    /// What it is.
    pub kind: NodeKind,
    /// The other version of it, while it is in conflict.
    pub other: Option<NodeId>,
}

/// One file's part of a state: where it stands, bytes moved and its size while it moves.
struct Part {
    state: ItemState,
    moved: u64,
    size: u64,
    reason: Option<Reason>,
}

impl Part {
    fn plain(state: ItemState) -> Self {
        Part {
            state,
            moved: 0,
            size: 0,
            reason: None,
        }
    }

    fn fraction(&self) -> Option<f64> {
        matches!(self.state, ItemState::Downloading | ItemState::Uploading).then(|| {
            match self.size {
                0 => 0.0,
                size => self.moved as f64 / size as f64,
            }
        })
    }
}

impl Inner {
    /// Whether `file`'s current version is all here.
    pub(super) fn here(&self, file: &Entry) -> bool {
        held(&self.index)
            .items
            .get(&file.id.0)
            .is_some_and(|row| row.version == file.version.0 && row.whole())
    }

    /// The state of what was changed here of `id`, while something of it is still to be sent.
    fn change_part(&self, id: &NodeId) -> Option<Part> {
        if self.local_only(id) {
            return Some(Part::plain(ItemState::Downloaded));
        }
        if self.other(id).is_some() {
            return Some(Part::plain(ItemState::Conflict));
        }
        let edit = self.edit(id)?;
        if let Some(reason) = edit.refused {
            return Some(Part {
                reason: Some(reason),
                ..Part::plain(ItemState::Error)
            });
        }
        if !edit.pending() {
            return None;
        }
        if let Some(sent) = held(&self.sending).as_ref()
            && sent.id == *id
        {
            return Some(Part {
                state: ItemState::Uploading,
                moved: sent.sent,
                size: sent.size,
                reason: None,
            });
        }
        let reason = held(&self.waiting).get(id).copied();
        Some(Part {
            reason,
            ..Part::plain(ItemState::Waiting)
        })
    }

    fn part(&self, file: &Entry) -> Part {
        if let Some(part) = self.change_part(&NodeId::Remote(file.id.clone())) {
            return part;
        }
        if let Some(transfer) = self.transfer(&file.id)
            && transfer.version == file.version.0
        {
            return Part {
                state: ItemState::Downloading,
                moved: transfer.moved_bytes(),
                size: transfer.size,
                reason: None,
            };
        }
        if let Some(reason) = held(&self.errors).get(&file.id).map(|f| f.reason) {
            return Part {
                reason: Some(reason),
                ..Part::plain(ItemState::Error)
            };
        }
        Part::plain(match self.here(file) {
            true => ItemState::Downloaded,
            false => ItemState::Cloud,
        })
    }

    /// The part of a node as shown.
    fn node_part(&self, node: &Node) -> Option<Part> {
        match &node.id {
            NodeId::Remote(remote) => {
                let entry = self.entry(remote).ok().flatten()?;
                entry.kind.is_file().then(|| self.part(&entry))
            }
            local @ NodeId::Local(_) => Some(
                self.change_part(local)
                    .unwrap_or_else(|| Part::plain(ItemState::Waiting)),
            ),
            NodeId::Top => None,
        }
    }

    /// A folder's state from what is under it as shown, with what was changed here of the
    /// folders themselves.
    fn folder_state(&self, folder: &NodeId) -> (ItemState, Option<f64>) {
        let mut parts = Vec::new();
        let mut queue = vec![folder.clone()];
        let mut seen = 0usize;
        while let Some(at) = queue.pop() {
            seen += 1;
            if seen > 100_000 {
                break;
            }
            if let Some(part) = self.change_part(&at)
                && part.state != ItemState::Downloaded
            {
                parts.push(part);
            }
            for node in self.children(&at).unwrap_or_default() {
                match node.kind {
                    NodeKind::Folder => queue.push(node.id.clone()),
                    NodeKind::File => parts.extend(self.node_part(&node)),
                    NodeKind::Online { .. } => {}
                }
            }
        }
        aggregate(&parts)
    }
}

/// A folder's state from its files': a conflict or an error when any is, then uploading or
/// downloading (with the share of the moving bytes that moved) while any is, waiting while any
/// waits, downloaded when all are, else cloud.
fn aggregate(parts: &[Part]) -> (ItemState, Option<f64>) {
    let any = |state: ItemState| parts.iter().any(|part| part.state == state);
    for state in [ItemState::Conflict, ItemState::Error] {
        if any(state) {
            return (state, None);
        }
    }
    for state in [ItemState::Uploading, ItemState::Downloading] {
        if any(state) {
            let moving = parts.iter().filter(|part| part.state == state);
            let (moved, size) =
                moving.fold((0u64, 0u64), |(m, s), part| (m + part.moved, s + part.size));
            let fraction = match size {
                0 => 0.0,
                size => moved as f64 / size as f64,
            };
            return (state, Some(fraction));
        }
    }
    if any(ItemState::Waiting) {
        return (ItemState::Waiting, None);
    }
    if !parts.is_empty() && parts.iter().all(|part| part.state == ItemState::Downloaded) {
        return (ItemState::Downloaded, None);
    }
    (ItemState::Cloud, None)
}

impl Drive {
    /// Where `id` stands; `None` for the top of the drive and for what is not there.
    pub fn status(&self, id: &NodeId) -> Option<Status> {
        let inner = &self.inner;
        let id = inner.canonical(id);
        let kept = |inner: &Inner, id: &NodeId| match id {
            NodeId::Remote(remote) if inner.edit(id).is_none() => {
                inner.kept_in(&held(&inner.index), remote)
            }
            NodeId::Remote(remote) => {
                held(&inner.index).kept.contains(&remote.0) || inner.kept_by_folder(id)
            }
            other => inner.kept_by_folder(other),
        };
        let attrs = inner.attrs(&id).ok().flatten()?;
        if id == NodeId::Top {
            return None;
        }
        let (state, fraction, reason) = match &attrs.kind {
            NodeKind::Folder => {
                let (state, fraction) = inner.folder_state(&id);
                (state, fraction, None)
            }
            NodeKind::File => {
                let part = match &id {
                    NodeId::Remote(remote) => {
                        let entry = inner.entry(remote).ok().flatten()?;
                        inner.part(&entry)
                    }
                    other => inner
                        .change_part(other)
                        .unwrap_or_else(|| Part::plain(ItemState::Waiting)),
                };
                (part.state, part.fraction(), part.reason)
            }
            NodeKind::Online { .. } => match inner.change_part(&id) {
                Some(part) => (part.state, part.fraction(), part.reason),
                None => (ItemState::Cloud, None, None),
            },
        };
        let other = match state {
            ItemState::Conflict => inner.other(&id).map(|other| inner.canonical(&other)),
            _ => None,
        };
        Some(Status {
            state,
            fraction,
            reason,
            kept: !matches!(attrs.kind, NodeKind::Online { .. }) && kept(inner, &id),
            kind: attrs.kind,
            other,
        })
    }
}
