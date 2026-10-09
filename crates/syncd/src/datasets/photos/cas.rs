//! Content addressing: an original is stored under the SHA-256 of its bytes, so two imports of
//! the same photo are one file, locally and on the replica, and a name can be checked against
//! the bytes it holds. Also the atomic file writes every part of the library uses.

use porter_fs::atomic::AtomicWrite;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::Path;

/// A photo's identity: the SHA-256 of its bytes, 64 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentId(String);

impl ContentId {
    /// The id, if `text` is one.
    pub fn parse(text: &str) -> Option<Self> {
        let ok = text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        ok.then(|| Self(text.to_owned()))
    }

    /// The id of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self::from_digest(Sha256::digest(bytes).as_slice())
    }

    fn from_digest(digest: &[u8]) -> Self {
        Self(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// The 64 hex digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Where the original is under the library's `originals` directory and under the replica's
    /// folder: `ab/abcdef...` (two digits of fan-out keep a directory small).
    pub fn rel(&self) -> String {
        format!("{}/{}", &self.0[..2], self.0)
    }

    /// The id a relative path `ab/<id>` names, if it names one (the shard must be the id's own).
    pub fn of_rel(rel: &str) -> Option<Self> {
        let (shard, name) = rel.split_once('/')?;
        let id = Self::parse(name)?;
        (id.0.starts_with(shard) && shard.len() == 2).then_some(id)
    }
}

impl TryFrom<String> for ContentId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text).ok_or_else(|| format!("not a content id: {text:?}"))
    }
}

impl From<ContentId> for String {
    fn from(id: ContentId) -> String {
        id.0
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The id and size of the file at `path`, read in chunks (a photo is never held whole).
pub fn hash_file(path: &Path) -> io::Result<(ContentId, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((ContentId::from_digest(hasher.finalize().as_slice()), size))
}

/// Writes `bytes` to `target` so that a reader sees the old file or the whole new one, through
/// porter's one atomic writer: a staging file (a dot file named `.tmp-*`, which no scan lists)
/// beside it, synced, renamed over it. Missing directories are made.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    AtomicWrite::SHARED.write(target, bytes)
}

/// Copies `source` to `target` the same way.
pub fn copy_atomic(source: &Path, target: &Path) -> io::Result<()> {
    AtomicWrite::SHARED.copy(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn an_id_is_64_lowercase_hex_and_its_path_carries_its_own_shard() {
        let id = ContentId::of(b"abc");
        assert_eq!(
            id.as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            id.rel(),
            "ba/ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(ContentId::of_rel(&id.rel()), Some(id.clone()));
        let upper = id.as_str().to_uppercase();
        let wrong_shard = format!("00/{id}");
        let nested = format!("ba/x/{id}");
        let bad: [&str; 7] = [
            "",
            "ba",
            &upper,
            "ba/short",
            &wrong_shard,
            &nested,
            "../etc/passwd",
        ];
        for rel in bad {
            assert_eq!(ContentId::of_rel(rel), None, "{rel:?}");
        }
        assert_eq!(ContentId::parse(&upper), None);
    }

    #[test]
    fn hashing_a_file_matches_hashing_its_bytes_and_counts_them() {
        let dir = scratch("cas-hash");
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("big"), &bytes).expect("file");
        assert_eq!(
            hash_file(&dir.join("big")).expect("hash"),
            (ContentId::of(&bytes), 200_000)
        );
        assert!(hash_file(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_atomic_write_replaces_whole_and_leaves_no_temp_file() {
        let dir = scratch("cas-atomic");
        let target = dir.join("a/b/file");
        write_atomic(&target, b"one").expect("first");
        write_atomic(&target, b"two").expect("second");
        assert_eq!(std::fs::read(&target).expect("read"), b"two");
        std::fs::write(dir.join("src"), b"three").expect("src");
        copy_atomic(&dir.join("src"), &target).expect("copy");
        assert_eq!(std::fs::read(&target).expect("read"), b"three");
        assert!(copy_atomic(&dir.join("missing"), &target).is_err());
        assert_eq!(
            std::fs::read(&target).expect("read"),
            b"three",
            "a failed copy changes nothing"
        );
        let names: Vec<_> = std::fs::read_dir(target.parent().expect("dir"))
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("file")]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
