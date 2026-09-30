//! Needs for the data kinds. Every field is a minimum in its enum's order.

use crate::capability::{Access, Albums, Delta, LibraryRead, Offered, QuotaReport, StorageScope};
use crate::units::Bytes;
use serde::{Deserialize, Serialize};

/// An identity with at least these parts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdentityNeed {
    /// A display name and avatar.
    pub profile: Offered,
    /// A verified address.
    pub verified_address: Offered,
}

/// A mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailNeed {
    /// At least this access.
    pub access: Access,
    /// Sending, if `Present`.
    pub send: Offered,
    /// At least this change feed.
    pub delta: Delta,
}

/// A calendar, address book or task store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PimNeed {
    /// At least this access.
    pub access: Access,
    /// At least this change feed.
    pub delta: Delta,
}

/// A notes store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NotesNeed {
    /// At least this access.
    pub access: Access,
    /// At least this change feed.
    pub delta: Delta,
}

/// A file store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StorageNeed {
    /// At least this access.
    pub access: Access,
    /// At least this change feed.
    pub delta: Delta,
    /// At least this reach: `AppFolder` accepts either scope, `Full` only a full one.
    pub scope: StorageScope,
    /// A quota report, if `Reported`.
    pub quota: QuotaReport,
}

/// A provider photo library.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhotosNeed {
    /// At least this much of the existing library.
    pub library_read: LibraryRead,
    /// Upload, if `Present`.
    pub upload: Offered,
    /// At least these albums.
    pub albums: Albums,
    /// Video, if `Present`.
    pub video: Offered,
    /// At least this change feed.
    pub delta: Delta,
}

/// Small synced items.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyValueNeed {
    /// At least this change feed.
    pub delta: Delta,
    /// Items at least this large.
    pub max_item: Bytes,
}

/// Any push channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PushNeed {}
