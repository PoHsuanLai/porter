//! The drive's listing as the core reads it: entries by id, each with its parent and its own
//! name, never a path.

use porter_ids::{Bytes, UnixSeconds};
use porter_sync::{ContentHash, ItemKind, RemoteId, RemoteVersion};
use std::collections::{BTreeSet, VecDeque};

/// How deep a folder walk goes before it is given up as a loop.
const DEPTH: usize = 64;

/// One entry of the drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The drive's id for it.
    pub id: RemoteId,
    /// The folder it is in; `None` at the top of the drive (the drive's root is no entry).
    pub parent: Option<RemoteId>,
    /// Its name in that folder, as the drive has it (it may hold a `/`).
    pub name: String,
    /// What it is.
    pub kind: ItemKind,
    /// Its size (a folder's is 0).
    pub size: Bytes,
    /// The drive's version of it: a cached copy of another version is stale.
    pub version: RemoteVersion,
    /// The drive's hash of that version's bytes ([`crate::Source::hashes`] says of which kind),
    /// when it reports one: a download is checked against it.
    pub hash: Option<ContentHash>,
    /// When it last changed, when the drive says.
    pub modified: Option<UnixSeconds>,
}

/// The listing could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the drive's listing could not be read: {0}")]
pub struct ListingError(pub String);

/// A drive's listing, read by id.
pub trait Listing: Send + Sync {
    /// The entry the drive knows as `id`.
    ///
    /// # Errors
    /// The listing could not be read.
    fn entry(&self, id: &RemoteId) -> Result<Option<Entry>, ListingError>;

    /// The entries directly inside `folder` (`None`: the top of the drive), in any order.
    ///
    /// # Errors
    /// The listing could not be read.
    fn children(&self, folder: Option<&RemoteId>) -> Result<Vec<Entry>, ListingError>;

    /// Every entry below `folder`, at any depth. The default walks [`Listing::children`]; a
    /// listing that can answer in one query does.
    ///
    /// # Errors
    /// The listing could not be read.
    fn descendants(&self, folder: &RemoteId) -> Result<Vec<Entry>, ListingError> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([(folder.clone(), 0usize)]);
        while let Some((at, depth)) = queue.pop_front() {
            if depth >= DEPTH || !seen.insert(at.clone()) {
                continue;
            }
            for child in self.children(Some(&at))? {
                if child.kind == ItemKind::Folder {
                    queue.push_back((child.id.clone(), depth + 1));
                }
                out.push(child);
            }
        }
        Ok(out)
    }

    /// The folders above `id`, nearest first, up to the top of the drive.
    ///
    /// # Errors
    /// The listing could not be read.
    fn ancestors(&self, id: &RemoteId) -> Result<Vec<RemoteId>, ListingError> {
        let mut out = Vec::new();
        let mut at = self.entry(id)?.and_then(|entry| entry.parent);
        while let Some(parent) = at {
            if out.len() >= DEPTH || out.contains(&parent) {
                break;
            }
            at = self.entry(&parent)?.and_then(|entry| entry.parent);
            out.push(parent);
        }
        Ok(out)
    }
}
