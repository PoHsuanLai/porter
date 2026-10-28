//! A content file's hash in the form its drive reports one (porter-sync's `ContentHash`,
//! lowercase hex): MD5 for Google Drive, QuickXorHash for OneDrive, SHA-256 where a drive
//! reports that. Other kinds are not computed here; a fetch of such a drive is checked by its
//! version instead.

use porter_capability::HashKind;
use quickxorhash::QuickXorHash;
use sha2::Digest;
use std::io::Read;
use std::path::Path;

/// Whether this crate computes hashes of `kind`.
pub(crate) fn computes(kind: HashKind) -> bool {
    matches!(kind, HashKind::Md5 | HashKind::Sha256 | HashKind::QuickXor)
}

/// One of the hashes computed here, being computed.
enum Hasher {
    Md5(md5::Md5),
    Sha256(sha2::Sha256),
    QuickXor(QuickXorHash),
}

impl Hasher {
    fn of(kind: HashKind) -> Option<Self> {
        match kind {
            HashKind::Md5 => Some(Hasher::Md5(md5::Md5::new())),
            HashKind::Sha256 => Some(Hasher::Sha256(sha2::Sha256::new())),
            HashKind::QuickXor => Some(Hasher::QuickXor(QuickXorHash::new())),
            _ => None,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::Md5(h) => h.update(bytes),
            Hasher::Sha256(h) => h.update(bytes),
            Hasher::QuickXor(h) => h.update(bytes),
        }
    }

    fn hex(self) -> String {
        match self {
            Hasher::Md5(h) => hex::encode(h.finalize()),
            Hasher::Sha256(h) => hex::encode(h.finalize()),
            Hasher::QuickXor(h) => hex::encode(h.finalize()),
        }
    }
}

/// The `kind` hash of `bytes`; `None` for a kind not computed here.
#[cfg(any(test, feature = "testing"))]
pub(crate) fn of_bytes(kind: HashKind, bytes: &[u8]) -> Option<String> {
    let mut hasher = Hasher::of(kind)?;
    hasher.update(bytes);
    Some(hasher.hex())
}

/// The `kind` hash of the file at `path`, read a piece at a time; `None` for a kind not
/// computed here.
pub(crate) fn of_file(kind: HashKind, path: &Path) -> std::io::Result<Option<String>> {
    let Some(mut hasher) = Hasher::of(kind) else {
        return Ok(None);
    };
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        match file.read(&mut buffer)? {
            0 => return Ok(Some(hasher.hex())),
            n => hasher.update(&buffer[..n]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_hashed_as_its_drive_reports_it() {
        assert_eq!(
            of_bytes(HashKind::Md5, b"").as_deref(),
            Some("d41d8cd98f00b204e9800998ecf8427e")
        );
        assert_eq!(
            of_bytes(HashKind::Sha256, b"abc").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(
            of_bytes(HashKind::QuickXor, b"J").as_deref(),
            Some("4a00000000000000000000000100000000000000")
        );
        assert_eq!(of_bytes(HashKind::None, b"x"), None);
        assert!(!computes(HashKind::Dropbox));
        let dir = test_kit::Scratch::new("hash");
        let path = dir.join("f");
        std::fs::write(&path, vec![7u8; 200_000]).expect("write");
        assert_eq!(
            of_file(HashKind::Md5, &path).expect("read"),
            of_bytes(HashKind::Md5, &[7u8; 200_000])
        );
    }
}
