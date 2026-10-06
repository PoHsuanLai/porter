//! A dataset is the local side of a sync: what is held here, how to read it, how to store what
//! the replica has. The engine runs one over a [`porter_sync::Replica`]; the real ones (the PIM
//! mirror W6e, Photos W6f) implement this trait, and tests implement it over memory.

#[cfg(any(test, feature = "testing"))]
mod memory;

#[cfg(any(test, feature = "testing"))]
pub use memory::MemoryDataset;

use porter_core::capability::HashKind;
use porter_sync::{Blob, ConflictRule, ContentHash, ItemPath, LocalId, Scanned};
use sha2::{Digest, Sha256};
use std::future::Future;

/// A dataset's name: a slug (`photos_originals`, `pim`), one path component and one word of a
/// `Sync1` dataset name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DatasetId(String);

impl DatasetId {
    /// The slug, if it is one: `[a-z0-9_]+`.
    pub fn parse(slug: &str) -> Option<Self> {
        let ok = !slug.is_empty()
            && slug.len() <= 48
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        ok.then(|| Self(slug.to_owned()))
    }

    /// The slug.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DatasetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The local side could not do what the engine asked (a file it cannot read or write).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("dataset: {0}")]
pub struct DatasetError(pub String);

/// Which way a dataset's items travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Both: local changes are uploaded and replica changes stored (the engine's default).
    #[default]
    TwoWay,
    /// Replica to local only (a mirror): the engine neither scans for local changes nor uploads
    /// or removes anything, and a changed item overwrites its local copy even if that was
    /// edited, never a conflict.
    PullOnly,
}

/// The local side of one dataset.
pub trait Dataset: Send + Sync {
    /// Its name.
    fn id(&self) -> DatasetId;

    /// How it settles a conflict.
    fn conflict_rule(&self) -> ConflictRule;

    /// Which way its items travel.
    fn direction(&self) -> Direction {
        Direction::TwoWay
    }

    /// Everything held locally now, each with its content's [`fingerprint`] (SHA-256 hex, always:
    /// the engine compares it with its own reading of the bytes): the engine diffs this against
    /// the journal to find local changes.
    fn scan(&self) -> impl Future<Output = Result<Vec<Scanned>, DatasetError>> + Send;

    /// One item's bytes.
    fn read(&self, item: &LocalId) -> impl Future<Output = Result<Blob, DatasetError>> + Send;

    /// Stores what the replica has at `path`: over `at` when the item is already held, else as
    /// a new item. Idempotent: storing the same content again changes nothing.
    fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> impl Future<Output = Result<Scanned, DatasetError>> + Send;

    /// Drops an item the replica deleted. Dropping one already gone is not an error.
    fn discard(&self, item: &LocalId) -> impl Future<Output = Result<(), DatasetError>> + Send;
}

impl<T: Dataset + ?Sized> Dataset for std::sync::Arc<T> {
    fn id(&self) -> DatasetId {
        (**self).id()
    }

    fn conflict_rule(&self) -> ConflictRule {
        (**self).conflict_rule()
    }

    fn direction(&self) -> Direction {
        (**self).direction()
    }

    fn scan(&self) -> impl Future<Output = Result<Vec<Scanned>, DatasetError>> + Send {
        (**self).scan()
    }

    fn read(&self, item: &LocalId) -> impl Future<Output = Result<Blob, DatasetError>> + Send {
        (**self).read(item)
    }

    fn store(
        &self,
        at: Option<&LocalId>,
        path: &ItemPath,
        content: Blob,
    ) -> impl Future<Output = Result<Scanned, DatasetError>> + Send {
        (**self).store(at, path, content)
    }

    fn discard(&self, item: &LocalId) -> impl Future<Output = Result<(), DatasetError>> + Send {
        (**self).discard(item)
    }
}

/// SHA-256 of `bytes`, in hex: the fingerprint datasets report in [`Scanned`] and the hash the
/// engine sends and compares for a replica that reports `HashKind::Sha256`.
pub fn fingerprint(bytes: &[u8]) -> ContentHash {
    let digest = Sha256::digest(bytes);
    ContentHash(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// The hash of `bytes` in the form a replica reports, when this build can compute it (SHA-256
/// only); otherwise the engine compares bytes.
pub fn replica_hash(kind: HashKind, bytes: &[u8]) -> Option<ContentHash> {
    match kind {
        HashKind::Sha256 => Some(fingerprint(bytes)),
        HashKind::None
        | HashKind::Md5
        | HashKind::Sha1
        | HashKind::QuickXor
        | HashKind::Dropbox => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dataset_name_is_a_plain_slug() {
        const CASES: &[(&str, bool)] = &[
            ("pim", true),
            ("photos_originals", true),
            ("", false),
            ("Photos", false),
            ("a/b", false),
            ("a b", false),
            ("a.b", false),
        ];
        for (text, ok) in CASES {
            assert_eq!(DatasetId::parse(text).is_some(), *ok, "{text:?}");
        }
    }

    #[test]
    fn the_fingerprint_is_sha256_hex_and_only_sha256_is_a_replica_hash() {
        assert_eq!(
            fingerprint(b"abc").0,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            replica_hash(HashKind::Sha256, b"abc"),
            Some(fingerprint(b"abc"))
        );
        for kind in [
            HashKind::None,
            HashKind::Md5,
            HashKind::Sha1,
            HashKind::QuickXor,
            HashKind::Dropbox,
        ] {
            assert_eq!(replica_hash(kind, b"abc"), None, "{kind:?}");
        }
    }
}
