//! Why a change or an action on a drive's items did not go through, as a code. The words a
//! person reads for each are written by whoever shows them (syncd, for Files), not here.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why an action did not take an item, or why a change or a download is not done. More reasons
/// may be added: match with a wildcard. `Display` says what happened, in terms of the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Reason {
    /// A path on no drive.
    #[error("the item is on no drive")]
    NotOnADrive,
    /// An online document asked to download, keep or remove.
    #[error("the item is an online document")]
    OnlineOnly,
    /// A download asked for while the drive cannot be reached, or one it refused.
    #[error("the drive cannot be reached for the download")]
    Offline,
    /// A removal asked for an item with changes not sent yet.
    #[error("the item has changes not sent yet")]
    Waiting,
    /// A change the drive refused: it has no room left.
    #[error("the drive has no room left")]
    NoRoom,
    /// A change the drive refused: the account may not change the item there.
    #[error("the account may not change the item on the drive")]
    MayNotWrite,
    /// A change the drive could not take now: the item is open or checked out there.
    #[error("the item is in use on the drive")]
    InUse,
    /// A change the drive refused: it does not take the item's name, or the path is too long.
    #[error("the drive does not take the item's name")]
    NameRefused,
    /// A change the drive refused for a reason of its own.
    #[error("the drive declined the change")]
    Declined,
    /// A file to send could not be copied for its upload: this computer's disk is full.
    #[error("there is no room on this computer to copy the file for its upload")]
    NoRoomHere,
    /// A removal asked for an item with two versions.
    #[error("the item has two versions")]
    Conflict,
    /// Keep Downloaded off, or a removal, asked for an item inside a kept folder.
    #[error("the item is kept downloaded with its folder")]
    KeptWithFolder,
    /// Keep Downloaded off, or a removal, asked while the whole drive is kept downloaded.
    #[error("the whole drive is kept downloaded")]
    KeptWithDrive,
    /// Keeping a whole drive downloaded needs more room than this computer has free.
    #[error("there is not enough free space to keep the whole drive downloaded")]
    NoRoomForAll,
    /// A drive id that names no drive.
    #[error("no such drive")]
    NoDrive,
    /// Save a Copy asked for an item that is not an online document.
    #[error("the item is not an online document")]
    NotADocument,
    /// Save a Copy asked for a Form, Site or Map, which have no file format.
    #[error("the document has no file format")]
    NoFileFormat,
    /// Save a Copy asked for a document larger than the drive exports.
    #[error("the document is larger than the drive exports")]
    TooLarge,
    /// Save a Copy that could not be made now (no network, or the drive did not answer).
    #[error("the copy could not be made")]
    CopyFailed,
}

impl Reason {
    /// Every reason, for tables.
    pub const ALL: [Reason; 19] = [
        Reason::NotOnADrive,
        Reason::OnlineOnly,
        Reason::Offline,
        Reason::Waiting,
        Reason::NoRoom,
        Reason::MayNotWrite,
        Reason::InUse,
        Reason::NameRefused,
        Reason::Declined,
        Reason::NoRoomHere,
        Reason::Conflict,
        Reason::KeptWithFolder,
        Reason::KeptWithDrive,
        Reason::NoRoomForAll,
        Reason::NoDrive,
        Reason::NotADocument,
        Reason::NoFileFormat,
        Reason::TooLarge,
        Reason::CopyFailed,
    ];

    /// The reason as a stable word: how the index keeps it.
    pub fn slug(self) -> &'static str {
        match self {
            Reason::NotOnADrive => "not_on_a_drive",
            Reason::OnlineOnly => "online_only",
            Reason::Offline => "offline",
            Reason::Waiting => "waiting",
            Reason::NoRoom => "no_room",
            Reason::MayNotWrite => "may_not_write",
            Reason::InUse => "in_use",
            Reason::NameRefused => "name_refused",
            Reason::Declined => "declined",
            Reason::NoRoomHere => "no_room_here",
            Reason::Conflict => "conflict",
            Reason::KeptWithFolder => "kept_with_folder",
            Reason::KeptWithDrive => "kept_with_drive",
            Reason::NoRoomForAll => "no_room_for_all",
            Reason::NoDrive => "no_drive",
            Reason::NotADocument => "not_a_document",
            Reason::NoFileFormat => "no_file_format",
            Reason::TooLarge => "too_large",
            Reason::CopyFailed => "copy_failed",
        }
    }

    /// The reason a word stands for.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.slug() == slug)
    }
}

impl Serialize for Reason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.slug())
    }
}

/// Reads a kept reason; text that is no reason of this build (a refusal an older build kept as
/// a sentence, or a newer build's) reads as none. A refusal is only held until the change is
/// tried again, which is what none means.
pub(crate) fn read_kept<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Reason>, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?;
    Ok(text.as_deref().and_then(Reason::from_slug))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reason_is_read_back_from_its_word() {
        for reason in Reason::ALL {
            assert_eq!(Reason::from_slug(reason.slug()), Some(reason));
        }
        assert_eq!(
            Reason::from_slug("There's no room left on this drive."),
            None
        );
    }

    #[test]
    fn no_reason_says_a_sentence_for_a_person() {
        for reason in Reason::ALL {
            let text = reason.to_string();
            assert!(!text.ends_with('.'), "{text}");
            assert!(text.starts_with(|c: char| c.is_ascii_lowercase()), "{text}");
        }
    }
}
