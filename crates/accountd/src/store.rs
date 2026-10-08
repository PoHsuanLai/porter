//! accountd's registry file (porter PLAN §2.3): `registry.json` in the state directory the
//! caller names, the serde form of [`Persisted`]. The directory is an argument, never read from
//! the environment here; the binary resolves `$XDG_STATE_HOME/porter` once.
//!
//! A save is atomic: the document goes to a temporary file in the same directory (a name of its
//! own, so two saves never share one), is synced, renamed over the old one, and the directory is
//! synced. A crash before the rename leaves the old file whole. A file that cannot be read as a
//! registry is refused with a typed error and is never written over.
//!
//! Only blocking file calls live here; the seam is async, so each runs on tokio's blocking pool.

use porter_core::store::{Persisted, StoreFault};
use porter_service::{RegistryStore, StoreError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The registry file's name inside the state directory.
const FILE: &str = "registry.json";
/// What a save's staging file name starts with; the process and a number follow.
const STAGING: &str = "registry.json.tmp";

/// Numbers the staging files of this process.
static STAGED: AtomicU64 = AtomicU64::new(0);

/// The registry kept as one JSON file, mode 0600 where the platform has modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// A store in `dir` (created on the first save).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// The registry file.
    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// A staging name no other save, in this process or another, is using.
    fn staging(&self) -> PathBuf {
        let n = STAGED.fetch_add(1, Ordering::Relaxed);
        self.dir
            .join(format!("{STAGING}-{}-{n}", std::process::id()))
    }

    /// The first half of a save: the whole document on disk and synced, under a staging name of
    /// its own, which it returns. Until [`FileStore::commit`] the registry file is untouched.
    fn stage(&self, text: &str) -> io::Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        let staged = self.staging();
        let written = private_options().open(&staged).and_then(|mut file| {
            file.write_all(text.as_bytes())
                .and_then(|()| file.sync_all())
        });
        match written {
            Ok(()) => Ok(staged),
            Err(e) => {
                let _ = std::fs::remove_file(&staged);
                Err(e)
            }
        }
    }

    /// The second half: the staged document replaces the registry file, and the directory entry
    /// is made durable.
    fn commit(&self, staged: &Path) -> io::Result<()> {
        let done = std::fs::rename(staged, self.path());
        if done.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        done?;
        sync_dir(&self.dir)
    }

    fn load_blocking(&self) -> Result<Persisted, StoreError> {
        match std::fs::read(self.path()) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|e| StoreFault::Unreadable(e.utf8_error().to_string()))?;
                Ok(Persisted::from_json(&text)?)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Persisted::empty()),
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
        self.stage(&text)
            .and_then(|staged| self.commit(&staged))
            .map_err(|_| StoreError::Unavailable)
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

/// Create a file that is not there yet, readable by the owner alone.
fn private_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    // Windows cannot open a directory as a file; the rename is as durable as it gets there.
    Ok(())
}

#[cfg(test)]
mod tests;
