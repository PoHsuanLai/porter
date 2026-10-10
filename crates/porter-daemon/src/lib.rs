//! What the desktop's daemons share and need no bus, runtime or environment for.
//!
//! Two small pieces, each used by several daemons that used to carry a copy:
//!
//! - [`ProcRoot`]: which `/proc` a daemon reads its callers from. The system's, unless a test
//!   build is told to read a fixture tree. Which process a caller is decides what it may do, so
//!   an environment variable must never be able to move this in a shipped build: the build says
//!   whether it honours the variable ([`ProcGate`]), and a shipped build does not.
//! - [`Watch`]: a watch on one settings file, which survives the atomic rename an editor or the
//!   Settings app writes it with.
//!
//! The library reads no environment variable. The daemon passes in how it looks one up, so a
//! test (or a program that runs a daemon inside itself) passes a table instead.
//!
//! ```
//! use std::ffi::OsString;
//! use porter_daemon::{ProcGate, ProcRoot};
//!
//! // The daemon's own build decides the gate; here, a test build.
//! let lookup = |name: &str| (name == "MYD_PROC_ROOT").then(|| OsString::from("/fixture/proc"));
//! let root = ProcRoot::choose(ProcGate::Honour, "MYD_PROC_ROOT", lookup);
//! assert_eq!(root.path(), std::path::Path::new("/fixture/proc"));
//!
//! // A shipped build ignores the same variable.
//! let root = ProcRoot::choose(ProcGate::Ignore, "MYD_PROC_ROOT", lookup);
//! assert_eq!(root.path(), std::path::Path::new("/proc"));
//! assert!(root.notice("myd", "MYD_PROC_ROOT").is_some());
//! ```

mod proc_root;
mod watch;

pub use proc_root::{ProcGate, ProcRoot};
pub use watch::{Watch, WatchError};
