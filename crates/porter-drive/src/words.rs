//! The words a refused action or a drive's state gives, as Files shows them. The fake of
//! `org.quire.Storage1` (porter-rig) gives the same words: it re-exports these.

/// A path on no drive.
pub const NOT_ON_A_DRIVE: &str = "This item isn't in a cloud drive.";
/// An online document asked to download, keep or remove.
pub const ONLINE_ONLY: &str = "This document opens online.";
/// A download asked for while the drive cannot be reached, or one it refused.
pub const OFFLINE: &str = "This file will download when you're connected.";
/// A removal asked for an item with changes not sent yet.
pub const WAITING: &str = "Waiting to upload.";
/// A change the drive refused: it has no room left.
pub const NO_ROOM: &str = "There's no room left on this drive.";
/// A change the drive refused: the account may not change the item there.
pub const MAY_NOT_WRITE: &str = "You can't change this file.";
/// A removal asked for an item with two versions.
pub const CONFLICT: &str = "This file has two versions. Choose one first.";
/// Keep Downloaded off, or a removal, asked for an item inside a kept folder.
pub const KEPT_WITH_FOLDER: &str = "This item is kept downloaded with its folder.";
/// A drive id that names no drive.
pub const NO_DRIVE: &str = "This drive isn't in Files.";
/// Save a Copy asked for an item that is not an online document.
pub const NOT_A_DOCUMENT: &str = "Only an online document can be saved as a copy.";
/// Save a Copy asked for a Form, Site or Map, which have no file format.
pub const NO_FILE_FORMAT: &str = "This document can't be saved as a file.";
/// Save a Copy asked for a document larger than the drive exports.
pub const TOO_LARGE: &str = "This document is too large to save as a copy.";
/// Save a Copy that could not be made now (no network, or the drive did not answer).
pub const COPY_FAILED: &str = "The copy couldn't be made. Try again later.";
/// A drive this computer cannot show as a folder (no way to show files on demand here).
pub const NO_FILES_HERE: &str = "Files can't show this drive on this computer.";
