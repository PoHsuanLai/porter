//! Storage as syncd runs it: the supervisor that turns a grant on a Microsoft or Google account
//! into a running mirror (the daemon half of W6c, Photos W6f's Graph wiring, and the Google
//! Drive app data folder and Google Photos).
//!
//! - `folder`: [`FolderDataset`], a plain directory as a two-way dataset.
//! - `grants`: which accounts syncd holds a Storage grant for ([`StorageGrants`]).
//! - `supervisor`: [`StorageSupervisor`], one mirror per granted account and kind
//!   ([`StorageKind`]): the app folder at `$XDG_DATA_HOME/porter/storage/<account>/` (OneDrive's
//!   or Drive's, by the endpoint the account has), the Photos datasets over `Photos/Originals`
//!   and `Photos/Metadata` of a Microsoft account, or Google Photos' upload folder and picker
//!   for a Google one; and the whole drive's listing
//!   ([`crate::datasets::drive`]) of an account the person turned on "Show in Files" for.
//!
//! A person needs: a Microsoft or Google account in accountd, and a grant for `org.quire.Sync`
//! made in Settings (class Files for the app folder, class Photos for Photos: the account's
//! Photos row is the only thing that turns Photos on).

mod folder;
mod grants;
mod supervisor;

pub use folder::{FolderDataset, SLUG as APP_FOLDER_SLUG};
pub use grants::{
    AppFolderStore, ClientStorageGrants, StorageGrants, StorageKind, app_folder_store, drive_need,
    google_photos_endpoints, graph_endpoint, photos_need, storage_need,
};
pub use supervisor::{DriveMount, FILES_APP, PHOTOS_APP, StorageConfig, StorageSupervisor};
