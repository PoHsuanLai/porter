//! `org.quire.Storage1` at `/org/quire/Storage1`, on syncd's connection (`org.quire.Sync1`): each
//! storage account's drive as a folder every app reaches at a real path, for Files and the shell.
//! A drive's files show before they are downloaded and download when opened; a person keeps a
//! file or folder downloaded, or removes a download to free space.
//!
//! A drive is one storage account the person turned on for Files ("Show in Files"). Its `path` is
//! porter's to choose and is only ever read from `Drives`: a caller never builds it. Every path
//! this interface takes or gives is absolute and inside one drive's `path`.
//!
//! Who may call: the shell (`CallerRole::SheetHost`), Settings (`CallerRole::Settings`) and an app
//! the person let see their files (per-app consent, as any app asks porter). Anyone else, and a
//! sender syncd does not know, is `AccessDenied`. A caller sees only the drives it was let see.

use crate::args::Details;
use std::collections::HashMap;
use zbus::fdo;
use zbus::object_server::SignalEmitter;

/// A drive row's account: its object path under `/org/quire/Accounts1/account/` (`o`).
pub const DRIVE_KEY_ACCOUNT: &str = "account";
/// A drive row's account label, the person's words for it (`s`).
pub const DRIVE_KEY_LABEL: &str = "label";
/// A drive row's provider name as people say it, "OneDrive", "Google Drive" (`s`).
pub const DRIVE_KEY_PROVIDER: &str = "provider";
/// A drive row's folder, where every app reaches its files (`s`).
pub const DRIVE_KEY_PATH: &str = "path";
/// A drive row's state ([`DriveState`], `s`).
pub const DRIVE_KEY_STATE: &str = "state";
/// What went wrong, in words a person reads, when the state is `error` or `needs_sign_in` (`s`).
pub const DRIVE_KEY_WORDS: &str = "words";
/// Bytes the account uses (`t`); absent when the provider does not say.
pub const DRIVE_KEY_USED: &str = "used";
/// Bytes the account may use (`t`); absent when unknown or unlimited.
pub const DRIVE_KEY_TOTAL: &str = "total";

/// An item's state ([`ItemState`], `s`).
pub const ITEM_KEY_STATE: &str = "state";
/// Whether the person keeps it downloaded (`b`): true while it is kept, whatever else it is
/// doing (a kept file that is uploading is `uploading` and kept).
pub const ITEM_KEY_KEPT: &str = "kept";
/// How far a transfer is, 0 to 1 (`d`), while the state is `downloading` or `uploading`.
pub const ITEM_KEY_FRACTION: &str = "fraction";
/// What went wrong, in words a person reads, when the state is `error` (`s`).
pub const ITEM_KEY_WORDS: &str = "words";
/// What the item is ([`ItemKind`], `s`).
pub const ITEM_KEY_KIND: &str = "kind";

/// Where a drive stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DriveState {
    /// Turned on and its listing not read yet: the folder is there and fills.
    Starting,
    /// Up to date.
    Ready,
    /// Reading changes, or sending them.
    Syncing,
    /// No network: what is downloaded opens, the rest waits.
    Offline,
    /// The person paused it.
    Paused,
    /// The account needs signing in again (or the person has not yet agreed to the access Files
    /// needs); `words` says so.
    NeedsSignIn,
    /// Something else is wrong; `words` says what.
    Error,
}

impl DriveState {
    /// Every state, in declaration order.
    pub const ALL: [DriveState; 7] = [
        DriveState::Starting,
        DriveState::Ready,
        DriveState::Syncing,
        DriveState::Offline,
        DriveState::Paused,
        DriveState::NeedsSignIn,
        DriveState::Error,
    ];

    /// Its word on the bus.
    pub fn as_str(self) -> &'static str {
        match self {
            DriveState::Starting => "starting",
            DriveState::Ready => "ready",
            DriveState::Syncing => "syncing",
            DriveState::Offline => "offline",
            DriveState::Paused => "paused",
            DriveState::NeedsSignIn => "needs_sign_in",
            DriveState::Error => "error",
        }
    }

    /// The state a word names; `None` for one this build does not know (a newer syncd's), which a
    /// caller shows as it shows `Error`.
    pub fn from_word(word: &str) -> Option<DriveState> {
        DriveState::ALL
            .into_iter()
            .find(|state| state.as_str() == word)
    }
}

/// Where one file or folder stands. A folder's is its own: `downloading` or `uploading` while any
/// item under it is, `downloaded` when everything under it is, else `cloud`, and `conflict` or
/// `error` when any item under it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ItemState {
    /// Listed, not downloaded: it downloads when it is opened.
    Cloud,
    /// Downloading; `fraction` says how far.
    Downloading,
    /// Downloaded and the same as the drive's copy.
    Downloaded,
    /// Changed here and being sent; `fraction` says how far.
    Uploading,
    /// Changed here and not yet sent ("Waiting to upload"): offline, paused, or queued.
    Waiting,
    /// It could not be downloaded or sent; `words` says why.
    Error,
    /// Changed here and on the drive both: both versions are kept, side by side.
    Conflict,
}

impl ItemState {
    /// Every state, in declaration order.
    pub const ALL: [ItemState; 7] = [
        ItemState::Cloud,
        ItemState::Downloading,
        ItemState::Downloaded,
        ItemState::Uploading,
        ItemState::Waiting,
        ItemState::Error,
        ItemState::Conflict,
    ];

    /// Its word on the bus.
    pub fn as_str(self) -> &'static str {
        match self {
            ItemState::Cloud => "cloud",
            ItemState::Downloading => "downloading",
            ItemState::Downloaded => "downloaded",
            ItemState::Uploading => "uploading",
            ItemState::Waiting => "waiting",
            ItemState::Error => "error",
            ItemState::Conflict => "conflict",
        }
    }

    /// The state a word names; `None` for one this build does not know.
    pub fn from_word(word: &str) -> Option<ItemState> {
        ItemState::ALL
            .into_iter()
            .find(|state| state.as_str() == word)
    }
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ItemKind {
    /// A file with content.
    File,
    /// A folder.
    Folder,
    /// A document that lives only online (a Google Doc, Sheet or Slides): the file is a link that
    /// opens it in the browser, never downloaded, kept or removed.
    Online,
}

impl ItemKind {
    /// Every kind, in declaration order.
    pub const ALL: [ItemKind; 3] = [ItemKind::File, ItemKind::Folder, ItemKind::Online];

    /// Its word on the bus.
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::File => "file",
            ItemKind::Folder => "folder",
            ItemKind::Online => "online",
        }
    }

    /// The kind a word names; `None` for one this build does not know.
    pub fn from_word(word: &str) -> Option<ItemKind> {
        ItemKind::ALL.into_iter().find(|kind| kind.as_str() == word)
    }
}

/// Each item's details by its path (`a{sa{sv}}`): what `Items` answers and `ItemsChanged` carries.
pub type ItemDetails = HashMap<String, Details>;

/// The paths an action did not take, each with words a person reads (`a{ss}`): empty when it took
/// them all.
pub type Refused = HashMap<String, String>;

/// The caller's side.
#[zbus::proxy(
    interface = "org.quire.Storage1",
    default_service = "org.quire.Sync1",
    default_path = "/org/quire/Storage1"
)]
pub trait Storage {
    /// The drives the caller may see, one row each: the drive's id (stable while the account is
    /// in porter), then `account`, `label`, `provider`, `path`, `state`, and `words`, `used` and
    /// `total` when there is something to say (the `DRIVE_KEY_*` names). A drive is listed from
    /// the moment the person turns it on, `starting` until its listing is read.
    fn drives(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Where each of `paths` stands: `state`, `kept` and `kind` always, `fraction` while it
    /// moves, `words` on an error (the `ITEM_KEY_*` names). A path on no drive the caller sees,
    /// or that is not there, is left out of the answer.
    fn items(&self, paths: Vec<String>) -> zbus::Result<ItemDetails>;
    /// "Download Now": downloads each of `paths` (a folder: everything under it). Answers when
    /// the downloads are queued; `ItemsChanged` tells how they go. A path already downloaded is
    /// taken and nothing happens; an online document, or a path on no drive, is refused.
    fn download(&self, paths: Vec<String>) -> zbus::Result<Refused>;
    /// "Keep Downloaded" on (`keep` true: downloads each of `paths` and never removes it to free
    /// space) or off (false: it stays downloaded until space is needed). Refuses an online
    /// document and a path on no drive.
    fn keep(&self, paths: Vec<String>, keep: bool) -> zbus::Result<Refused>;
    /// "Remove Download": frees the space each of `paths` takes here; it stays listed and
    /// downloads again when opened. Turns Keep Downloaded off for it. Refuses an item with
    /// changes not yet sent ("Waiting to upload") or uploading, so nothing made here is lost.
    fn remove_download(&self, paths: Vec<String>) -> zbus::Result<Refused>;
    /// Joins the caller to the connections syncd tells: a connection hears `DrivesChanged` and
    /// `ItemsChanged` only after calling this, and then only for the drives it may see.
    fn watch(&self) -> zbus::Result<()>;
    /// The drives changed (one turned on or off, a state, the space used). No payload: a
    /// listener calls `Drives` again.
    #[zbus(signal)]
    fn drives_changed(&self) -> zbus::Result<()>;
    /// Items of `drive` changed, each with its details as `Items` gives them. A changed item's
    /// folders are in it too when theirs changed. While a transfer moves, each drive's changes
    /// are sent together up to ten times a second, so a progress pie fills smoothly.
    #[zbus(signal)]
    fn items_changed(&self, drive: &str, items: ItemDetails) -> zbus::Result<()>;
}

/// The daemon's side.
#[derive(Debug, Default)]
pub struct StorageSkeleton;

#[zbus::interface(name = "org.quire.Storage1")]
impl StorageSkeleton {
    fn drives(&self) -> fdo::Result<Vec<(String, Details)>> {
        Err(crate::introspect::frozen())
    }

    fn items(&self, paths: Vec<String>) -> fdo::Result<ItemDetails> {
        let _ = paths;
        Err(crate::introspect::frozen())
    }

    fn download(&self, paths: Vec<String>) -> fdo::Result<Refused> {
        let _ = paths;
        Err(crate::introspect::frozen())
    }

    fn keep(&self, paths: Vec<String>, keep: bool) -> fdo::Result<Refused> {
        let _ = (paths, keep);
        Err(crate::introspect::frozen())
    }

    fn remove_download(&self, paths: Vec<String>) -> fdo::Result<Refused> {
        let _ = paths;
        Err(crate::introspect::frozen())
    }

    fn watch(&self) -> fdo::Result<()> {
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn drives_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn items_changed(
        emitter: &SignalEmitter<'_>,
        drive: &str,
        items: ItemDetails,
    ) -> zbus::Result<()>;
}
