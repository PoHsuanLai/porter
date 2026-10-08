//! accountd's registry file (porter PLAN §2.3): `registry.json` in the state directory the
//! caller names, the serde form of [`Persisted`]. The directory is an argument, never read from
//! the environment here; the binary resolves `$XDG_STATE_HOME/porter` once.
//!
//! A save is atomic: the document goes to a temporary file in the same directory (a name of its
//! own, so two saves never share one), is synced, renamed over the old one, and the directory is
//! synced. A crash before the rename leaves the old file whole. Each commit first keeps the file
//! it replaces as `registry.json.bak`, the last file that loaded. A file that cannot be read as a
//! registry is refused with a typed error and is never written over; neither is the `.bak`.
//! The file handling is [`file::AtomicFile`], which `spaces.json` shares.
//!
//! Only blocking file calls live here; the seam is async, so each runs on tokio's blocking pool.

pub(crate) mod file;

use file::AtomicFile;
use porter_core::store::{Persisted, StoreFault};
use porter_service::{RegistryStore, StoreError};
use std::io;
use std::path::PathBuf;

/// The registry file's name inside the state directory.
const FILE: &str = "registry.json";

/// The exit status of a daemon that refused the registry file (EX_CONFIG). `accountd.service`
/// names it in `RestartPreventExitStatus=`, so the unit stops instead of restarting.
pub const EXIT_REGISTRY_REFUSED: u8 = 78;

/// The registry kept as one JSON file, mode 0600 where the platform has modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStore {
    file: AtomicFile,
}

impl FileStore {
    /// A store in `dir` (created on the first save).
    pub fn new(dir: PathBuf) -> Self {
        Self {
            file: AtomicFile::new(dir, FILE),
        }
    }

    /// The registry file.
    pub fn path(&self) -> PathBuf {
        self.file.path()
    }

    /// The copy of the registry file from before its last change.
    pub fn backup_path(&self) -> PathBuf {
        self.file.backup_path()
    }

    /// What to tell the person when the registry file is refused at start: both files by name,
    /// and that nothing was changed. The daemon stops (it never starts empty over a file it could
    /// not read) and nothing is restored on its own.
    pub fn refusal(&self, why: &impl std::fmt::Display) -> String {
        let (file, backup) = (self.path(), self.backup_path());
        let spare = match backup.exists() {
            true => format!(
                "The copy from before the last change is {}. To go back to it, stop the accounts \
                 service, put that copy in place of the first file, and start it again.",
                backup.display()
            ),
            false => "There is no earlier copy.".to_string(),
        };
        format!(
            "the accounts file {} cannot be read ({why}). Nothing was changed and the accounts \
             service did not start, so no account is lost. {spare}",
            file.display()
        )
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
        // A registry file that does not load is never replaced by a save: the daemon has to
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
