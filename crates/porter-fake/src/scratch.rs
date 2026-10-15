//! Where a test's scratch directory lives.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Hands each guard in this process its own number.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A scratch directory for a test, `porter-<name>-<pid>-<n>` under the temp directory, created
/// empty and removed when the guard drops.
///
/// The temp directory is memory-backed; cargo's own scratch directory sits on the build disk,
/// whose flushes starve under load (a journal with synchronous writes took a scenario from half
/// a second to minutes, a daemon missed its start deadline). The temp directory is shared by
/// every checkout and every test run on the machine, so the name carries the process id; the
/// counter `n` keeps two guards in one process apart, so `name` is only a readable label and
/// need not be unique. A process id can be reused after a crash, so a directory already at the
/// path is removed before the new one is created. Memory-backed also means a directory nobody
/// removes stays in RAM until reboot, so dropping the guard removes it; keep the guard alive
/// as long as the directory is used, and drop anything that runs inside it (a daemon, a
/// child process) first.
///
/// It derefs to the directory's [`Path`].
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// A new, empty scratch directory labelled `name`.
    ///
    /// # Panics
    /// When the directory cannot be created.
    #[must_use]
    pub fn new(name: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("porter-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
