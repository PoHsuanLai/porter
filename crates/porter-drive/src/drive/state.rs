//! Where each item stands, as Files shows it: a file's own state, a folder's from what is under
//! it.

use super::{Drive, Inner, NodeId, NodeKind, held};
use crate::listing::Entry;
use porter_sync::{ItemKind, RemoteId};
use std::collections::BTreeSet;

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
    /// Made or changed here and not sent yet ("Waiting to upload").
    Waiting,
    /// It could not be downloaded; the words say why.
    Error,
}

/// An item's details.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Where it stands.
    pub state: ItemState,
    /// How far a download is, 0 to 1, while it is downloading.
    pub fraction: Option<f64>,
    /// What went wrong, when the state is an error.
    pub words: Option<String>,
    /// Whether it is kept downloaded, itself or with a folder above it.
    pub kept: bool,
    /// What it is.
    pub kind: NodeKind,
}

/// One file's part of a state: where it stands, bytes moved and its size while it downloads.
struct Part {
    state: ItemState,
    moved: u64,
    size: u64,
    words: Option<String>,
}

impl Inner {
    /// Whether `file`'s current version is all here.
    pub(super) fn here(&self, file: &Entry) -> bool {
        held(&self.index)
            .items
            .get(&file.id.0)
            .is_some_and(|row| row.version == file.version.0 && row.whole())
    }

    fn part(&self, file: &Entry) -> Part {
        if let Some(transfer) = self.transfer(&file.id)
            && transfer.version == file.version.0
        {
            return Part {
                state: ItemState::Downloading,
                moved: transfer.moved_bytes(),
                size: transfer.size,
                words: None,
            };
        }
        if let Some(words) = held(&self.errors).get(&file.id).map(|f| f.words.clone()) {
            return Part {
                state: ItemState::Error,
                moved: 0,
                size: 0,
                words: Some(words),
            };
        }
        Part {
            state: match self.here(file) {
                true => ItemState::Downloaded,
                false => ItemState::Cloud,
            },
            moved: 0,
            size: 0,
            words: None,
        }
    }

    fn folder_state(&self, folder: &RemoteId) -> (ItemState, Option<f64>) {
        let under = self.listing.descendants(folder).unwrap_or_default();
        let mut parts: Vec<Part> = under
            .iter()
            .filter(|entry| entry.kind.is_file())
            .map(|file| self.part(file))
            .collect();
        let folders: BTreeSet<&str> = std::iter::once(folder.0.as_str())
            .chain(
                under
                    .iter()
                    .filter(|e| e.kind == ItemKind::Folder)
                    .map(|e| e.id.0.as_str()),
            )
            .collect();
        let waiting = held(&self.index)
            .local
            .iter()
            .filter(|row| row.parent.as_deref().is_some_and(|p| folders.contains(p)))
            .count();
        parts.extend((0..waiting).map(|_| Part {
            state: ItemState::Waiting,
            moved: 0,
            size: 0,
            words: None,
        }));
        aggregate(&parts)
    }
}

/// A folder's state from its files': an error when any is, downloading (with the share of the
/// moving bytes that moved) while any is, waiting while any waits, downloaded when all are, else
/// cloud.
fn aggregate(parts: &[Part]) -> (ItemState, Option<f64>) {
    let any = |state: ItemState| parts.iter().any(|part| part.state == state);
    if any(ItemState::Error) {
        return (ItemState::Error, None);
    }
    if any(ItemState::Downloading) {
        let moving = parts
            .iter()
            .filter(|part| part.state == ItemState::Downloading);
        let (moved, size) =
            moving.fold((0u64, 0u64), |(m, s), part| (m + part.moved, s + part.size));
        let fraction = match size {
            0 => 0.0,
            size => moved as f64 / size as f64,
        };
        return (ItemState::Downloading, Some(fraction));
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
        let kept = |inner: &Inner, id: &NodeId| match id {
            NodeId::Remote(remote) => inner.kept_in(&held(&inner.index), remote),
            other => inner.kept_by_folder(other),
        };
        match id {
            NodeId::Top => None,
            NodeId::Local(n) => {
                inner.local_row(*n)?;
                Some(Status {
                    state: ItemState::Waiting,
                    fraction: None,
                    words: None,
                    kept: kept(inner, id),
                    kind: NodeKind::File,
                })
            }
            NodeId::Remote(remote) => {
                let entry = inner.entry(remote).ok().flatten()?;
                let kind = NodeKind::of(&entry.kind);
                let (state, fraction, words) = match &entry.kind {
                    ItemKind::Folder => {
                        let (state, fraction) = inner.folder_state(remote);
                        (state, fraction, None)
                    }
                    ItemKind::File => {
                        let part = inner.part(&entry);
                        let fraction =
                            (part.state == ItemState::Downloading).then(|| match part.size {
                                0 => 0.0,
                                size => part.moved as f64 / size as f64,
                            });
                        (part.state, fraction, part.words)
                    }
                    _ => (ItemState::Cloud, None, None),
                };
                Some(Status {
                    state,
                    fraction,
                    words,
                    kept: !matches!(kind, NodeKind::Online { .. }) && kept(inner, id),
                    kind,
                })
            }
        }
    }
}
