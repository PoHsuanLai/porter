//! The model catalog as porter sees it: stoker's `catalog/*.toml` files, read from the system
//! and user directories, become `Claim`s at `Provenance::Curated` for the `local` provider
//! (`Discovery::Supervised`). The parsing is stoker's `model-catalog`; this module is the
//! mapping, which waits for that crate's dependency edge.

use porter_core::{Claim, Provenance};
use std::path::PathBuf;

/// Where the catalog files live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDirs {
    /// `/usr/share/stoker/catalog`.
    pub system: PathBuf,
    /// `$XDG_DATA_HOME/stoker/catalog`; a file here replaces the system file of the same id.
    pub user: PathBuf,
}

/// The claims the catalog makes for the local account: one per model and capability, at the
/// provenance the catalog gives (`Curated`).
pub fn local_claims(dirs: &CatalogDirs) -> Vec<Claim> {
    let _ = (dirs, Provenance::Curated);
    todo!("merge_catalogs(system, user), then one Claim per model capability")
}
