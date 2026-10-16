//! Gets an engine onto this computer from `porter-engines`' pinned manifest.
//!
//! - [`Fetcher`] downloads one pinned file: over `https` only, resumable, cancellable, counted
//!   in a [`Meter`], and kept only if its size and SHA-256 are the pinned ones.
//! - [`Installer`] installs a manifest row for everyone or for one person: space checked, files
//!   fetched, unpacked with the system's `tar` (or a Python environment filled from the row's
//!   hashed lock), the tree's seal compared with the pinned one, then renamed into place whole.
//! - [`install_package`] installs the distro's package instead.
//! - [`EngineInstalls`] is what a program that offers installs holds: what could be installed for
//!   an engine here ([`InstallChoice`]), the jobs it starts (for the person in this process, for
//!   everyone through `pkexec` and the helper, [`install_for_everyone`]) and where each stands.
//! - [`helper`] is the privileged side: the only arguments it accepts (an engine and a backend
//!   by name, never an address, path or version), the report lines it prints and the work it
//!   does as root. The `porter-engine-helper` program is a thin shell around it.
//!
//! Nothing here chooses what to install or asks anyone: that is the caller's.

mod elevated;
mod extract;
mod fetch;
pub mod helper;
mod installer;
mod jobs;
mod package;
mod progress;
mod pyenv;
mod space;

pub use elevated::{HelperProgram, install_for_everyone};
pub use extract::{ExtractError, settle_modes, unpack};
pub use fetch::{FetchError, Fetcher};
pub use installer::{InstallError, Installer};
pub use jobs::{EngineInstalls, InstallChoice, Parts, Standing, StartError};
pub use package::install_package;
pub use progress::{CancelToken, Meter};
pub use space::{NoSpace, free_bytes, require_space};
