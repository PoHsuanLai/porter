//! Helpers the unit tests share: scratch directories and nothing else. Tests never touch
//! `~/.config`, `~/.local` or `/etc`.

use tempfile::TempDir;

/// A new empty scratch directory for one test, named after it and removed when dropped.
pub(crate) fn scratch(name: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(&format!("syncd-{name}-"))
        .tempdir()
        .expect("scratch")
}
