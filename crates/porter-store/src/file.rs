//! The registry file (porter PLAN §2.3): `registry.json` in the directory the host names, the
//! serde form of [`Persisted`]. The directory is an argument, never read from the environment
//! here; accountd's binary resolves `$XDG_STATE_HOME/porter` once, and an app
//! names its own.
//!
//! A save is atomic: the document goes to a temporary file in the same directory (a name of its
//! own, so two saves never share one), is synced, renamed over the old one, and the directory is
//! synced. A crash before the rename leaves the old file whole. Each commit first keeps the file
//! it replaces as `registry.json.bak`, the last file that loaded. A file that cannot be read as a
//! registry is refused with a typed error and is never written over; neither is the `.bak`.
//! The file handling is [`AtomicFile`], which accountd's `spaces.json` shares.
//!
//! Only blocking file calls live here; the seam is async, so each runs on tokio's blocking pool.

use porter_core::store::{Persisted, StoreFault};
use porter_core::store::{RegistryStore, StoreError};
use porter_fs::atomic::AtomicFile;
use std::io;
use std::path::{Path, PathBuf};

/// The registry file's name inside the store's directory.
const FILE: &str = "registry.json";

/// The registry kept as one JSON file, mode 0600 where the platform has modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStore {
    dir: PathBuf,
    file: AtomicFile,
}

/// Why the registry file was refused at start: which file, the copy from before its last
/// change when there is one, and what was wrong with it. The words for a person are the
/// host's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("registry file {} refused: {cause}", file.display())]
#[non_exhaustive]
pub struct RegistryRefusal {
    file: PathBuf,
    backup: Option<PathBuf>,
    cause: String,
}

impl RegistryRefusal {
    /// The registry file.
    pub fn file(&self) -> &std::path::Path {
        &self.file
    }

    /// The copy from before its last change, when one is there.
    pub fn backup(&self) -> Option<&std::path::Path> {
        self.backup.as_deref()
    }

    /// What was wrong with the file.
    pub fn cause(&self) -> &str {
        &self.cause
    }
}

impl FileStore {
    /// A store in `dir` (created on the first save). The host names the directory: accountd's
    /// `Config::from_env` resolves `$XDG_STATE_HOME/porter`, and an app names its own.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            file: AtomicFile::new(dir.clone(), FILE),
            dir,
        }
    }

    /// The directory the store keeps its files in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The registry file.
    pub fn path(&self) -> PathBuf {
        self.file.path()
    }

    /// The copy of the registry file from before its last change.
    pub fn backup_path(&self) -> PathBuf {
        self.file.backup_path()
    }

    /// The refusal of the registry file at start, for the host to word: both files and why.
    /// The host stops (it never starts empty over a file it could not read) and nothing is
    /// restored on its own.
    pub fn refusal(&self, why: &impl std::fmt::Display) -> RegistryRefusal {
        let backup = self.backup_path();
        RegistryRefusal {
            file: self.path(),
            backup: backup.exists().then_some(backup),
            cause: why.to_string(),
        }
    }

    /// A staging name no other save, in this process or another, is using.
    #[cfg(test)]
    fn staging(&self) -> PathBuf {
        self.file.staging()
    }

    /// The first half of a save, alone (a crash before the rename).
    #[cfg(test)]
    fn stage(&self, text: &str) -> io::Result<PathBuf> {
        self.file.stage(text)
    }

    fn load_blocking(&self) -> Result<Persisted, StoreError> {
        match self.file.read() {
            Ok(Some(bytes)) => {
                let text = String::from_utf8(bytes)
                    .map_err(|e| StoreFault::Unreadable(e.utf8_error().to_string()))?;
                Ok(Persisted::from_json(&text)?)
            }
            Ok(None) => Ok(Persisted::empty()),
            Err(_) => Err(StoreError::Unavailable),
        }
    }

    fn save_blocking(&self, state: &Persisted) -> Result<(), StoreError> {
        let text = state.to_json()?;
        // A registry file that does not load is never replaced by a save: the host has to
        // have refused it at start, and a later save must not slip past that.
        if let Err(StoreError::Fault(fault)) = self.load_blocking() {
            return Err(StoreError::Fault(fault));
        }
        self.file
            .write(&text)
            .map_err(|_: io::Error| StoreError::Unavailable)
    }
}

impl RegistryStore for FileStore {
    async fn load(&self) -> Result<Persisted, StoreError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.load_blocking())
            .await
            .map_err(|_| StoreError::Unavailable)?
    }

    async fn save(&self, state: &Persisted) -> Result<(), StoreError> {
        let store = self.clone();
        let state = state.clone();
        tokio::task::spawn_blocking(move || store.save_blocking(&state))
            .await
            .map_err(|_| StoreError::Unavailable)?
    }
}

#[cfg(test)]
mod tests;
