//! A watch on one settings file.
//!
//! The watch is on the directory that holds the file, not on the file: the Settings app's atomic
//! writer and every editor replace the file's inode by renaming a new one over it, which a watch
//! on the old inode never sees. Events for other files in the directory, and plain reads of the
//! file (reading it to apply it must not re-arm the watch), are dropped.
//!
//! The watch calls a function the daemon gives, on a thread of the watcher's own. The function
//! should only hand the news on (send on a channel, wake a task) and return; the daemon debounces
//! and reads the file again on its own side, since one rename can fire more than one event.
//!
//! ```no_run
//! use std::sync::mpsc;
//! use porter_daemon::Watch;
//!
//! # fn main() -> Result<(), porter_daemon::WatchError> {
//! let (changed, events) = mpsc::channel();
//! let dir = std::path::PathBuf::from("/home/me/.config/mydaemon");
//! // Dropping `watch` stops it.
//! let watch = Watch::start(dir, "settings.toml", move || {
//!     // The receiver is gone once the daemon stops; nobody is left to tell.
//!     let _ = changed.send(());
//! })?;
//! events.recv().ok(); // the file changed: read it again
//! drop(watch);
//! # Ok(())
//! # }
//! ```

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Why a settings file could not be watched.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WatchError {
    /// The directory that holds the file could not be made.
    #[error("{}: {source}", dir.display())]
    CreateDir {
        /// The directory.
        dir: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The system's file watcher refused (no more watches to give, for one).
    #[error("{0}")]
    Notify(#[from] notify::Error),
}

/// A running watch on one file. Dropping it stops the watch.
#[must_use = "dropping a Watch stops it"]
pub struct Watch {
    _watcher: RecommendedWatcher,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch").finish_non_exhaustive()
    }
}

/// Whether `event` writes the file called `file`: not an access, and not another file in the
/// directory.
fn touches(event: &notify::Event, file: &OsStr) -> bool {
    !event.kind.is_access()
        && event
            .paths
            .iter()
            .any(|path| path.file_name() == Some(file))
}

impl Watch {
    /// Watches the file called `file_name` in `dir`, making `dir` first if it is not there, and
    /// calls `on_change` for every event that writes it.
    ///
    /// # Errors
    /// `dir` could not be made, or the system's file watcher refused.
    pub fn start(
        dir: PathBuf,
        file_name: impl Into<OsString>,
        on_change: impl Fn() + Send + 'static,
    ) -> Result<Self, WatchError> {
        std::fs::create_dir_all(&dir).map_err(|source| WatchError::CreateDir {
            dir: dir.clone(),
            source,
        })?;
        let file = file_name.into();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && touches(&event, &file)
                {
                    on_change();
                }
            })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::EventKind;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RenameMode};
    use std::sync::mpsc;

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn only_a_write_of_the_file_itself_counts() {
        let file = OsStr::new("settings.toml");
        let rows = [
            (
                EventKind::Create(CreateKind::File),
                "/d/settings.toml",
                true,
            ),
            (
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                "/d/settings.toml",
                true,
            ),
            (
                EventKind::Access(AccessKind::Any),
                "/d/settings.toml",
                false,
            ),
            (EventKind::Create(CreateKind::File), "/d/other.toml", false),
            (
                EventKind::Create(CreateKind::File),
                "/d/settings.toml.tmp",
                false,
            ),
        ];
        for (kind, path, expected) in rows {
            assert_eq!(
                touches(&event(kind, path), file),
                expected,
                "{kind:?} {path}"
            );
        }
    }

    #[test]
    fn a_rename_over_the_file_is_heard_and_a_missing_directory_is_made() {
        let root = std::env::temp_dir().join(format!("porter-daemon-watch-{}", std::process::id()));
        let dir = root.join("nested");
        let (sender, heard) = mpsc::channel();
        let watch = Watch::start(dir.clone(), "settings.toml", move || {
            let _ = sender.send(());
        })
        .expect("watch");
        assert!(dir.is_dir());
        let staged = dir.join("settings.toml.new");
        std::fs::write(&staged, "x = 1\n").expect("stage");
        std::fs::rename(&staged, dir.join("settings.toml")).expect("replace");
        // Waits for the event with no clock: it arrives whenever the machine gets to it.
        heard.recv().expect("an event for the replaced file");
        drop(watch);
        let _ = std::fs::remove_dir_all(&root);
    }
}
