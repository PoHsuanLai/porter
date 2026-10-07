//! Storage over Microsoft Graph as syncd runs it (the daemon half of W6c, and Photos W6f's
//! Graph wiring): the supervisor that turns a Storage grant on a Microsoft account into a
//! running mirror.
//!
//! - `folder`: [`FolderDataset`], a plain directory as a two-way dataset.
//! - `grants`: which accounts syncd holds a Storage grant for ([`StorageGrants`]).
//! - `supervisor`: [`StorageSupervisor`], one mirror per granted account and kind
//!   ([`StorageKind`]): the app folder at `$XDG_DATA_HOME/porter/storage/<account>/`, and (behind
//!   the Photos switch) the Photos datasets over `Photos/Originals` and `Photos/Metadata`.
//!
//! A person needs: a Microsoft account in accountd, and a Storage grant for `org.quire.Sync`
//! made in Settings (class Files for the app folder, class Photos for Photos); for Photos also
//! `SYNCD_PHOTOS=on` in syncd's environment.

mod folder;
mod grants;
mod supervisor;

pub use folder::{FolderDataset, SLUG as APP_FOLDER_SLUG};
pub use grants::{ClientStorageGrants, StorageGrants, StorageKind, graph_endpoint, storage_need};
pub use supervisor::{FILES_APP, PHOTOS_APP, StorageConfig, StorageSupervisor};
