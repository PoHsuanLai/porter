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
//! Only the shell and Settings may pause a drive.
//!
//! An online document (a Google Doc, Sheet or Slides) is a small file on the drive, named with
//! its own extension (`.gdoc`, `.gsheet`, `.gslides`): a JSON object whose `url` member is the
//! document's address, as Google's own desktop client writes them. A caller opens the `url` of
//! its details rather than reading the file; an app that opens the file directly reads `url`.

use crate::Details;
use porter_dbus_macros::bus_interface;
use porter_ids::word::{from_word, word};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
/// Bytes of the drive's files downloaded on this computer (`t`), what "Remove Download" frees.
/// It moves when a download ends or is removed, not while one fills.
pub const DRIVE_KEY_LOCAL: &str = "local";

/// An item's state ([`ItemState`], `s`).
pub const ITEM_KEY_STATE: &str = "state";
/// Whether the person keeps it downloaded (`b`): true while it is kept, whatever else it is
/// doing (a kept file that is uploading is `uploading` and kept). Keeping a folder keeps
/// everything in it, also what is added to it later, so every item under a kept folder reads
/// true.
pub const ITEM_KEY_KEPT: &str = "kept";
/// How far a transfer is, 0 to 1 (`d`), while the state is `downloading` or `uploading`. A
/// folder's is the share of the bytes moving under it that have moved, so a caller never sums
/// its children.
pub const ITEM_KEY_FRACTION: &str = "fraction";
/// What went wrong, in words a person reads, when the state is `error` (`s`).
pub const ITEM_KEY_WORDS: &str = "words";
/// What the item is ([`ItemKind`], `s`).
pub const ITEM_KEY_KIND: &str = "kind";
/// An online document's address (`s`), always there when the kind is `online`: a caller opens
/// it with the OpenURI portal.
pub const ITEM_KEY_URL: &str = "url";
/// The other version of a file in `conflict` (`s`): its path, on the same drive, next to this
/// one. Each of the two names the other.
pub const ITEM_KEY_OTHER: &str = "other";

/// Where a drive stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Its word on the bus.
    pub fn as_str(self) -> &'static str {
        word(&self)
    }

    /// The state a word names; `None` for one this build does not know (a newer syncd's), which a
    /// caller shows as it shows `Error`.
    pub fn from_word(text: &str) -> Option<DriveState> {
        from_word(text)
    }
}

/// Where one file or folder stands. A folder's is its own: `downloading` or `uploading` while any
/// item under it is, `downloaded` when everything under it is, else `cloud`, and `conflict` or
/// `error` when any item under it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Changed here and on the drive both: both versions are kept, side by side, and `other`
    /// names the other one.
    Conflict,
}

impl ItemState {
    /// Its word on the bus.
    pub fn as_str(self) -> &'static str {
        word(&self)
    }

    /// The state a word names; `None` for one this build does not know.
    pub fn from_word(text: &str) -> Option<ItemState> {
        from_word(text)
    }
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Its word on the bus.
    pub fn as_str(self) -> &'static str {
        word(&self)
    }

    /// The kind a word names; `None` for one this build does not know.
    pub fn from_word(text: &str) -> Option<ItemKind> {
        from_word(text)
    }
}

/// Each item's details by its path (`a{sa{sv}}`): what `Items` answers and `ItemsChanged` carries.
pub type ItemDetails = HashMap<String, Details>;

/// The paths an action did not take, each with words a person reads (`a{ss}`): empty when it took
/// them all.
pub type Refused = HashMap<String, String>;

/// Each online document `SaveCopy` saved, by its path, with the path of the file it saved (`a{ss}`).
pub type Saved = HashMap<String, String>;

/// The interface, written once. `#[bus_interface]` makes [`StorageProxy`] for its callers from this
/// trait, [`StorageServer`] for the daemon to implement and [`StorageServed`] to register.
#[bus_interface(
    interface = "org.quire.Storage1",
    default_service = "org.quire.Sync1",
    default_path = "/org/quire/Storage1"
)]
pub trait Storage {
    /// The drives the caller may see, one row each: the drive's id (stable while the account is
    /// in porter), then `account`, `label`, `provider`, `path`, `state`, `local`, and `words`,
    /// `used` and `total` when there is something to say (the `DRIVE_KEY_*` names). A drive is
    /// listed from the moment the person turns it on, `starting` until its listing is read.
    fn drives(&self) -> zbus::Result<Vec<(String, Details)>>;
    /// Where each of `paths` stands: `state`, `kept` and `kind` always, `fraction` while it
    /// moves, `words` on an error, `url` for an online document and `other` on a conflict (the
    /// `ITEM_KEY_*` names). A path on no drive the caller sees,
    /// or that is not there, is left out of the answer.
    fn items(&self, paths: Vec<String>) -> zbus::Result<ItemDetails>;
    /// "Download Now": downloads each of `paths` (a folder: everything under it). Answers when
    /// the downloads are queued; `ItemsChanged` tells how they go. A path already downloaded is
    /// taken and nothing happens; an online document, or a path on no drive, is refused.
    fn download(&self, paths: Vec<String>) -> zbus::Result<Refused>;
    /// "Keep Downloaded" on (`keep` true: downloads each of `paths` and never removes it to free
    /// space) or off (false: it stays downloaded until space is needed). A kept folder keeps
    /// what is added to it later; turning a folder off turns off everything in it. Refuses an
    /// online document, a path on no drive, and turning off an item kept with its folder.
    fn keep(&self, paths: Vec<String>, keep: bool) -> zbus::Result<Refused>;
    /// "Remove Download": frees the space each of `paths` takes here; it stays listed and
    /// downloads again when opened. Turns Keep Downloaded off for it. Refuses an item with
    /// changes not yet sent ("Waiting to upload") or uploading, so nothing made here is lost, a
    /// file in conflict, and an item kept with its folder.
    fn remove_download(&self, paths: Vec<String>) -> zbus::Result<Refused>;
    /// "Save a Copy": saves each online document of `paths` as a file in its export format (a
    /// Doc as `.docx`, a Sheet as `.xlsx`, Slides as `.pptx`, a Drawing as `.png`) next to it,
    /// named as the document is: "Plan.docx", or "Plan 2.docx", "Plan 3.docx" when the name is
    /// taken. The copy is a downloaded file of the drive. Answers `saved`, each document's path
    /// with its copy's, and `refused`: a path that is not an online document, a Form, Site or
    /// Map (which have no file format), and a path on no drive.
    #[bus(out_args("saved", "refused"))]
    fn save_copy(&self, paths: Vec<String>) -> zbus::Result<(Saved, Refused)>;
    /// "Pause Syncing" (`paused` true) or "Resume Syncing" (false) for each drive id of
    /// `drives`. A paused drive sends nothing and reads no changes; a file the person opens or
    /// asks to download still downloads. Answers the ids it did not take, with words. The shell
    /// and Settings only.
    fn set_paused(&self, drives: Vec<String>, paused: bool) -> zbus::Result<Refused>;
    /// Joins the caller to the connections syncd tells: a connection hears `DrivesChanged` and
    /// `ItemsChanged` only after calling this, and then only for the drives it may see. It lasts
    /// until the connection closes; a caller that connects again calls it again.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_drive_state_keeps_the_word_it_has_always_had_on_the_bus() {
        let words = [
            (DriveState::Starting, "starting"),
            (DriveState::Ready, "ready"),
            (DriveState::Syncing, "syncing"),
            (DriveState::Offline, "offline"),
            (DriveState::Paused, "paused"),
            (DriveState::NeedsSignIn, "needs_sign_in"),
            (DriveState::Error, "error"),
        ];
        for (state, word) in words {
            assert_eq!(state.as_str(), word);
            assert_eq!(DriveState::from_word(word), Some(state));
        }
        assert_eq!(DriveState::from_word("NeedsSignIn"), None);
        assert_eq!(DriveState::from_word("later"), None);
    }

    #[test]
    fn each_item_state_keeps_the_word_it_has_always_had_on_the_bus() {
        let words = [
            (ItemState::Cloud, "cloud"),
            (ItemState::Downloading, "downloading"),
            (ItemState::Downloaded, "downloaded"),
            (ItemState::Uploading, "uploading"),
            (ItemState::Waiting, "waiting"),
            (ItemState::Error, "error"),
            (ItemState::Conflict, "conflict"),
        ];
        for (state, word) in words {
            assert_eq!(state.as_str(), word);
            assert_eq!(ItemState::from_word(word), Some(state));
        }
        assert_eq!(ItemState::from_word("Cloud"), None);
    }

    #[test]
    fn each_item_kind_keeps_the_word_it_has_always_had_on_the_bus() {
        let words = [
            (ItemKind::File, "file"),
            (ItemKind::Folder, "folder"),
            (ItemKind::Online, "online"),
        ];
        for (kind, word) in words {
            assert_eq!(kind.as_str(), word);
            assert_eq!(ItemKind::from_word(word), Some(kind));
        }
        assert_eq!(ItemKind::from_word("link"), None);
    }
}
