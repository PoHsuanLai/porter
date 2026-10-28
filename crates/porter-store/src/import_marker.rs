//! The record that one daemon's registry was copied into another's: what was copied, and the
//! digest of the file it was copied from. The daemon that copied writes it (`ai-import.json`
//! beside its audit file) after it verified the copy; the daemon that owns the old files reads it
//! and removes them only while their digest is still the one that was copied.

use porter_fs::atomic::AtomicWrite;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;
use std::path::Path;

/// What was copied from an old registry, and from which file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImportMarker {
    /// The SHA-256 of the old registry file, in lower-case hex.
    pub source_sha256: String,
    /// How many accounts the old registry held.
    pub accounts: usize,
    /// How many grants.
    pub grants: usize,
    /// How many switches.
    pub toggles: usize,
    /// How many secrets were copied (a count, never a value).
    pub secrets: usize,
    /// How many audit lines were appended.
    pub audit_lines: usize,
    /// Whether the old secrets were removed once the copy was verified.
    pub secrets_removed: bool,
}

/// Why a marker could not be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MarkerError {
    /// The file could not be read.
    #[error("the import record could not be read: {0:?}")]
    Io(io::ErrorKind),
    /// The file is not an import record.
    #[error("the import record does not parse")]
    Unreadable,
}

impl ImportMarker {
    /// A record of a copy of the registry file whose bytes are `source`.
    pub fn of(source: &[u8]) -> Self {
        Self {
            source_sha256: Self::digest(source),
            accounts: 0,
            grants: 0,
            toggles: 0,
            secrets: 0,
            audit_lines: 0,
            secrets_removed: false,
        }
    }

    /// The SHA-256 of `bytes`, in lower-case hex.
    pub fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Whether this record says the file whose bytes are `source` was copied.
    pub fn covers(&self, source: &[u8]) -> bool {
        self.source_sha256 == Self::digest(source)
    }

    /// The record at `path`; none when there is no such file.
    ///
    /// # Errors
    /// The file exists and cannot be read, or is not a record.
    pub fn read(path: &Path) -> Result<Option<Self>, MarkerError> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| MarkerError::Unreadable),
            Err(why) if why.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(why) => Err(MarkerError::Io(why.kind())),
        }
    }

    /// Writes the record to `path`, atomically, readable by the owner alone.
    ///
    /// # Errors
    /// The file could not be written.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        AtomicWrite::PRIVATE.write(path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_kit::Scratch;

    #[test]
    fn a_record_reads_back_and_covers_the_file_it_was_made_of_and_no_other() {
        let scratch = Scratch::new("import-marker");
        let path = scratch.join("ai-import.json");
        assert!(ImportMarker::read(&path).expect("absent").is_none());
        let mut record = ImportMarker::of(b"{\"vocab\":1}");
        record.accounts = 2;
        record.secrets_removed = true;
        record.write(&path).expect("written");
        let back = ImportMarker::read(&path).expect("read").expect("there");
        assert_eq!(back, record);
        assert!(back.covers(b"{\"vocab\":1}"));
        assert!(!back.covers(b"{\"vocab\":2}"));
        assert_eq!(back.source_sha256.len(), 64);
    }

    #[test]
    fn a_file_that_is_not_a_record_is_refused() {
        let scratch = Scratch::new("import-marker");
        let path = scratch.join("ai-import.json");
        std::fs::write(&path, "not json").expect("file");
        assert!(matches!(
            ImportMarker::read(&path),
            Err(MarkerError::Unreadable)
        ));
    }
}
