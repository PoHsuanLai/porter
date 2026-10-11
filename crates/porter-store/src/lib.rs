//! The registry file store, as a library: `registry.json` in a directory the host chooses, saved
//! atomically with the previous file kept as `registry.json.bak`, behind porter-service's
//! [`RegistryStore`](porter_service::RegistryStore) seam. accountd keeps its registry here, and
//! an app that hosts porter's core itself (porter-client's `InProcess`) keeps its own the same
//! way, with its own directory.
//!
//! The directory is a [`StoreLocation`] the host names; the library reads no environment and no
//! XDG variable. A file that cannot be read is refused and never written over:
//!
//! ```
//! use porter_store::{FileStore, StoreLocation};
//!
//! let store = FileStore::at(StoreLocation::new("/home/ada/.local/state/my-app"));
//! assert!(store.path().ends_with("my-app/registry.json"));
//! assert!(store.backup_path().ends_with("my-app/registry.json.bak"));
//! ```
//!
//! This crate is separate from porter-service because the file calls run on tokio's blocking
//! pool, and porter-service depends on no executor (`scripts/check-boundary.sh` holds it to that).

mod file;
mod location;

pub use file::FileStore;
pub use location::StoreLocation;
