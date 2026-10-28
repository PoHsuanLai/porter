//! What the desktop's daemons share and need no bus, runtime or environment for.
//!
//! Three small pieces, each used by several daemons that used to carry a copy:
//!
//! - [`Watch`]: a watch on one settings file, which survives the atomic rename an editor or the
//!   Settings app writes it with.
//! - [`xdg`]: the XDG base directories a daemon keeps its files under, resolved from the lookup
//!   the daemon passes in.
//! - [`registry_words`]: the words a registry file's errors are told in.
//!
//! The library reads no environment variable. The daemon passes in how it looks one up, so a
//! test (or a program that runs a daemon inside itself) passes a table instead. Which `/proc` and
//! which caller tables a daemon reads are in `porter-callers`.

mod registry;
#[cfg(feature = "watch")]
mod watch;
pub mod xdg;

pub use registry::registry_words;
#[cfg(feature = "watch")]
pub use watch::{Watch, WatchError};
