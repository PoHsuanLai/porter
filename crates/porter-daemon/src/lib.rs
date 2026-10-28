//! What the desktop's daemons share and need no bus, runtime or environment for.
//!
//! Three small pieces, each used by several daemons that used to carry a copy:
//!
//! - [`ProcRoot`]: which `/proc` a daemon reads its callers from. The system's, unless a test
//!   build is told to read a fixture tree. Which process a caller is decides what it may do, so
//!   an environment variable must never be able to move this in a shipped build: the build says
//!   whether it honours the variable, and a shipped build does not.
//! - [`Watch`]: a watch on one settings file, which survives the atomic rename an editor or the
//!   Settings app writes it with.
//! - [`xdg`]: the XDG base directories a daemon keeps its files under, resolved from the lookup
//!   the daemon passes in.
//!
//! The library reads no environment variable. The daemon passes in how it looks one up, so a
//! test (or a program that runs a daemon inside itself) passes a table instead.
//!
//! ```
//! use std::ffi::OsString;
//! use porter_daemon::{choose_proc_root, proc_notice, proc_path};
//!
//! // The daemon's own build decides whether it honours the variable; here, a test build.
//! let lookup = |name: &str| (name == "MYD_PROC_ROOT").then(|| OsString::from("/fixture/proc"));
//! let root = choose_proc_root(true, "MYD_PROC_ROOT", lookup);
//! assert_eq!(proc_path(&root), std::path::Path::new("/fixture/proc"));
//!
//! // A shipped build ignores the same variable.
//! let root = choose_proc_root(false, "MYD_PROC_ROOT", lookup);
//! assert_eq!(proc_path(&root), std::path::Path::new("/proc"));
//! assert!(proc_notice(&root, "myd", "MYD_PROC_ROOT").is_some());
//! ```

mod proc_root;
mod registry;
mod system_callers;
#[cfg(feature = "watch")]
mod watch;
pub mod xdg;

pub use proc_root::{Override, ProcRoot, choose_proc_root, proc_notice, proc_path};
pub use registry::registry_words;
pub use system_callers::{
    CALLERS, INFERD_CALLERS, SYSTEM_CALLERS, SYSTEM_CALLERS_VAR, SystemCallers, SystemFile,
    TABLE_DIRS, held_by, held_under, root_held,
};
#[cfg(feature = "watch")]
pub use watch::{Watch, WatchError};
