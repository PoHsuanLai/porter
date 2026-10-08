//! One small JSON file kept atomically, with the copy from before its last change: what the
//! registry (`registry.json`) and the desktop-wide Spaces (`spaces.json`) are both kept as.
//!
//! A save goes to a staging file in the same directory (`<name>.tmp-<pid>-<n>`, a name of its
//! own, so two saves never share one), is synced, and is renamed over the file; the directory is
//! then synced. A crash before the rename leaves the old file whole. Each commit first keeps the
//! file it replaces as `<name>.bak`. Blocking calls only.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Numbers the staging files of this process.
static STAGED: AtomicU64 = AtomicU64::new(0);

/// A file `name` in `dir`, mode 0600 where the platform has modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AtomicFile {
    dir: PathBuf,
    name: &'static str,
}

impl AtomicFile {
    /// The file `name` in `dir` (the directory is created on the first save).
    pub(crate) fn new(dir: PathBuf, name: &'static str) -> Self {
        Self { dir, name }
    }

    /// The file.
    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(self.name)
    }

    /// The copy of the file from before its last change.
    pub(crate) fn backup_path(&self) -> PathBuf {
        self.dir.join(format!("{}.bak", self.name))
    }

    /// The file's bytes; `None` when there is no file.
    pub(crate) fn read(&self) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.path()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replaces the file with `text`, atomically, keeping the one it replaces as the backup.
    pub(crate) fn write(&self, text: &str) -> io::Result<()> {
        self.stage(text).and_then(|staged| self.commit(&staged))
    }

    /// A staging name no other save, in this process or another, is using.
    pub(crate) fn staging(&self) -> PathBuf {
        let n = STAGED.fetch_add(1, Ordering::Relaxed);
        self.dir
            .join(format!("{}.tmp-{}-{n}", self.name, std::process::id()))
    }

    /// The first half of a save: the whole document on disk and synced, under a staging name of
    /// its own, which it returns. Until [`AtomicFile::commit`] the file is untouched.
    pub(crate) fn stage(&self, text: &str) -> io::Result<PathBuf> {
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

    /// The second half: the file being replaced is kept as the backup, the staged document
    /// replaces the file, and the directory entry is made durable.
    fn commit(&self, staged: &Path) -> io::Result<()> {
        let done = self
            .keep_previous()
            .and_then(|()| std::fs::rename(staged, self.path()));
        if done.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        done?;
        sync_dir(&self.dir)
    }

    /// Makes the backup the file as it is now, atomically: a copy under a staging name, synced,
    /// then renamed. Nothing to keep on the first save.
    fn keep_previous(&self) -> io::Result<()> {
        let copy = self.staging();
        let kept = match std::fs::read(self.path()) {
            Ok(bytes) => private_options()
                .open(&copy)
                .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
                .and_then(|()| std::fs::rename(&copy, self.backup_path())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => Err(e),
        };
        if kept.is_err() {
            let _ = std::fs::remove_file(&copy);
        }
        kept
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
