//! What has been sent to Google Photos: the album the app made and the SHA-256 of every file
//! uploaded, with the media item it became. The upload replica asks it before sending, so a
//! file already there (by content, whatever its name) is never sent again, also after a restart.
//! One small JSON file beside the account's journals, replaced whole.

use super::api::AlbumId;
use crate::datasets::photos::cas;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One file sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// SHA-256 of its bytes, hex.
    pub sha256: String,
    /// The media item it became.
    pub media: String,
    /// The name it was sent under.
    pub name: String,
}

/// The album and everything sent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// The album the app created, once it has.
    pub album: Option<AlbumId>,
    /// Every file sent, oldest first.
    pub entries: Vec<Entry>,
}

/// Why the ledger could not be read or kept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    /// The file exists and is not a ledger: nothing is sent until someone looks, since an
    /// empty ledger would send every file again.
    #[error("the upload ledger is unreadable: {0}")]
    Unreadable(String),
    /// The file could not be written.
    #[error("the upload ledger could not be saved: {0}")]
    Unsaved(String),
}

impl Ledger {
    /// The entry for content with this SHA-256.
    pub fn find(&self, sha256: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.sha256 == sha256)
    }

    /// The first entry for this media item.
    pub fn of_media(&self, media: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.media == media)
    }

    /// The ledger at `path`; none there is an empty one.
    pub fn load(path: &Path) -> Result<Self, LedgerError> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| LedgerError::Unreadable(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(LedgerError::Unreadable(e.to_string())),
        }
    }

    /// Keeps the ledger at `path`.
    pub fn save(&self, path: &Path) -> Result<(), LedgerError> {
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|e| LedgerError::Unsaved(e.to_string()))?;
        cas::write_atomic(path, &bytes).map_err(|e| LedgerError::Unsaved(e.to_string()))
    }
}

/// A ledger and where it is kept.
#[derive(Debug)]
pub(super) struct Kept {
    pub path: PathBuf,
    pub ledger: Ledger,
}

impl Kept {
    pub(super) fn open(path: PathBuf) -> Result<Self, LedgerError> {
        Ok(Self {
            ledger: Ledger::load(&path)?,
            path,
        })
    }

    pub(super) fn save(&self) -> Result<(), LedgerError> {
        self.ledger.save(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn a_ledger_survives_a_restart_and_a_damaged_one_is_not_an_empty_one() {
        let dir = scratch("ledger");
        let path = dir.join("sync/acct/google_photos_upload.ledger.json");
        assert_eq!(Ledger::load(&path), Ok(Ledger::default()));
        let ledger = Ledger {
            album: Some(AlbumId("album1".into())),
            entries: vec![Entry {
                sha256: "ab".into(),
                media: "m1".into(),
                name: "a.jpg".into(),
            }],
        };
        ledger.save(&path).expect("saved");
        let again = Ledger::load(&path).expect("loaded");
        assert_eq!(again, ledger);
        assert_eq!(again.find("ab").map(|e| e.media.as_str()), Some("m1"));
        assert_eq!(again.find("cd"), None);
        assert_eq!(again.of_media("m1").map(|e| e.name.as_str()), Some("a.jpg"));

        std::fs::write(&path, b"{not json").expect("damage");
        assert!(matches!(
            Ledger::load(&path),
            Err(LedgerError::Unreadable(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
