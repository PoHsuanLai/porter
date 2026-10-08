//! The one way porter writes a file that a reader or a crash must never see half written.
//!
//! A write goes to a staging file in the target's own directory (`.tmp-<pid>-<n>.tmp`, a dot
//! file whose name no other save, in this process or another, is using), is written whole and
//! synced to disk, is renamed over the target, and the directory is then synced so the rename
//! survives a power cut. A failure before the rename leaves the old file whole and removes the
//! staging file. Blocking calls only; the files are small.
//!
//! [`AtomicWrite`] is the writer; [`AtomicFile`] is one small file kept with the copy from before
//! its last change (`<name>.bak`), as the registry is. This module is the one place in porter
//! that touches the file system: the vocabulary around it stays free of I/O.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Numbers the staging files of this process.
static STAGED: AtomicU64 = AtomicU64::new(0);

/// What a write does about the directories above the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parents {
    /// Missing directories are made.
    Create,
    /// The directory must be there already: a write into one that is gone fails with `NotFound`
    /// and brings nothing back.
    MustExist,
}

/// The permissions a written file gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Readable and writable by the owner alone (mode 0600 where the platform has modes).
    OwnerOnly,
    /// The platform's default for a new file (the process's umask).
    Default,
}

/// A way of writing a file atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicWrite {
    parents: Parents,
    visibility: Visibility,
}

impl AtomicWrite {
    /// Owner-only files, missing directories made: credentials, settings, ledgers.
    pub const PRIVATE: Self = Self {
        parents: Parents::Create,
        visibility: Visibility::OwnerOnly,
    };

    /// Files with the default permissions, missing directories made: data other programs read.
    pub const SHARED: Self = Self {
        parents: Parents::Create,
        visibility: Visibility::Default,
    };

    /// The same writer, but it never makes a directory (see [`Parents::MustExist`]).
    pub const fn in_existing_dir(self) -> Self {
        Self {
            parents: Parents::MustExist,
            ..self
        }
    }

    /// Replaces `target` with `bytes`, atomically.
    pub fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<()> {
        self.publish(target, |file| file.write_all(bytes))
    }

    /// Replaces `target` with the contents of `source`, the same way.
    pub fn copy(&self, source: &Path, target: &Path) -> io::Result<()> {
        self.publish(target, |file| {
            io::copy(&mut std::fs::File::open(source)?, file).map(|_| ())
        })
    }

    /// The first half of a write: `bytes` on disk and synced under a staging name beside
    /// `target`, which it returns. Until [`AtomicWrite::commit`] the target is untouched.
    pub fn stage(&self, target: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        self.stage_with(target, |file| file.write_all(bytes))
    }

    /// The second half: the staged file replaces `target` and the directory entry is made
    /// durable. The staged file is removed if that fails.
    pub fn commit(&self, staged: &Path, target: &Path) -> io::Result<()> {
        let done = std::fs::rename(staged, target);
        if done.is_err() {
            let _ = std::fs::remove_file(staged);
        }
        done?;
        sync_dir(directory_of(target))
    }

    fn publish(
        &self,
        target: &Path,
        fill: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    ) -> io::Result<()> {
        let staged = self.stage_with(target, fill)?;
        self.commit(&staged, target)
    }

    fn stage_with(
        &self,
        target: &Path,
        fill: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        let dir = directory_of(target);
        if self.parents == Parents::Create {
            std::fs::create_dir_all(dir)?;
        }
        let staged = staging_beside(target);
        let written = self.options().open(&staged).and_then(|mut file| {
            fill(&mut file)?;
            sync_file(&file)
        });
        match written {
            Ok(()) => Ok(staged),
            Err(e) => {
                let _ = std::fs::remove_file(&staged);
                Err(e)
            }
        }
    }

    /// Create a file that is not there yet.
    fn options(&self) -> std::fs::OpenOptions {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if self.visibility == Visibility::OwnerOnly {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
    }
}

/// A staging name beside `target` that no other save is using. A dot file ending in `.tmp`, so
/// no scan for the real files lists it.
pub fn staging_beside(target: &Path) -> PathBuf {
    let n = STAGED.fetch_add(1, Ordering::Relaxed);
    directory_of(target).join(format!(".tmp-{}-{n}.tmp", std::process::id()))
}

/// The directory `target` is in (`.` for a bare file name).
fn directory_of(target: &Path) -> &Path {
    target
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn sync_file(file: &std::fs::File) -> io::Result<()> {
    #[cfg(test)]
    sync_count::file();
    file.sync_all()
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(test)]
    sync_count::dir();
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    // Windows cannot open a directory as a file; the rename is as durable as it gets there.
    Ok(())
}

/// A file `name` in `dir`, owner-only, kept with the copy from before its last change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicFile {
    dir: PathBuf,
    name: &'static str,
}

impl AtomicFile {
    /// The file `name` in `dir` (the directory is created on the first save).
    pub fn new(dir: PathBuf, name: &'static str) -> Self {
        Self { dir, name }
    }

    /// The file.
    pub fn path(&self) -> PathBuf {
        self.dir.join(self.name)
    }

    /// The copy of the file from before its last change.
    pub fn backup_path(&self) -> PathBuf {
        self.dir.join(format!("{}.bak", self.name))
    }

    /// The file's bytes; `None` when there is no file.
    pub fn read(&self) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.path()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replaces the file with `text`, atomically, keeping the one it replaces as the backup.
    pub fn write(&self, text: &str) -> io::Result<()> {
        let staged = self.stage(text)?;
        let kept = self.keep_previous();
        if let Err(e) = kept {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
        AtomicWrite::PRIVATE.commit(&staged, &self.path())
    }

    /// A staging name no other save, in this process or another, is using.
    pub fn staging(&self) -> PathBuf {
        staging_beside(&self.path())
    }

    /// The first half of a save: the whole document on disk and synced, under a staging name of
    /// its own, which it returns. Until the commit the file is untouched.
    pub fn stage(&self, text: &str) -> io::Result<PathBuf> {
        AtomicWrite::PRIVATE.stage(&self.path(), text.as_bytes())
    }

    /// Makes the backup the file as it is now, atomically. Nothing to keep on the first save.
    fn keep_previous(&self) -> io::Result<()> {
        match std::fs::read(self.path()) {
            Ok(bytes) => AtomicWrite::PRIVATE.write(&self.backup_path(), &bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// How many syncs this thread's writes made, so a test can see a write reach the disk.
#[cfg(test)]
pub(crate) mod sync_count {
    use std::cell::Cell;

    thread_local! {
        static FILES: Cell<usize> = const { Cell::new(0) };
        static DIRS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn file() {
        FILES.with(|n| n.set(n.get() + 1));
    }

    pub(super) fn dir() {
        DIRS.with(|n| n.set(n.get() + 1));
    }

    /// Files and directories synced on this thread so far.
    pub(crate) fn so_far() -> (usize, usize) {
        (FILES.with(Cell::get), DIRS.with(Cell::get))
    }
}

#[cfg(test)]
mod tests;
