//! Where a test's scratch directory lives.

use std::path::PathBuf;

/// A new scratch directory for a test, `porter-<name>-<pid>` under the temp directory, created.
///
/// The temp directory is memory-backed; cargo's own scratch directory sits on the build disk,
/// whose flushes starve under load (a journal with synchronous writes took a scenario from half
/// a second to minutes, a daemon missed its start deadline). The temp directory is shared by
/// every checkout and every test run on the machine, so the name carries the process id, and
/// `name` must be unique within one process (a counter, or the test's own name).
///
/// # Panics
/// When the directory cannot be created.
#[must_use]
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("porter-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}
