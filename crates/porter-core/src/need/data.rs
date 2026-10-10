//! Needs for the data kinds. Every field is a minimum in its enum's order.
//!
//! Each need grows by adding a minimum, so each is `#[non_exhaustive]` and built with `new`.

use crate::capability::{Access, Albums, Delta, LibraryRead, Offered, QuotaReport, StorageScope};
use crate::units::Bytes;
use serde::{Deserialize, Serialize};

/// An identity with at least these parts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IdentityNeed {
    /// A display name and avatar.
    pub profile: Offered,
    /// A verified address.
    pub verified_address: Offered,
}

impl IdentityNeed {
    /// An identity with these parts.
    pub fn new(profile: Offered, verified_address: Offered) -> Self {
        Self {
            profile,
            verified_address,
        }
    }
}

/// A mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MailNeed {
    /// At least this access.
    pub access: Access,
    /// Sending, if `Present`.
    pub send: Offered,
    /// At least this change feed.
    pub delta: Delta,
}

impl MailNeed {
    /// A mailbox with at least this access, sending if `send` is `Present`, and this change feed.
    pub fn new(access: Access, send: Offered, delta: Delta) -> Self {
        Self {
            access,
            send,
            delta,
        }
    }
}

/// A calendar, address book or task store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PimNeed {
    /// At least this access.
    pub access: Access,
    /// At least this change feed.
    pub delta: Delta,
}

impl PimNeed {
    /// A calendar, address book or task store with at least this access and change feed.
    pub fn new(access: Access, delta: Delta) -> Self {
        Self { access, delta }
    }
}

/// A notes store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct NotesNeed {
    /// At least this access.
    pub access: Access,
    /// At least this change feed.
    pub delta: Delta,
}

impl NotesNeed {
    /// A notes store with at least this access and change feed.
    pub fn new(access: Access, delta: Delta) -> Self {
        Self { access, delta }
    }
}

/// A file store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
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

impl StorageNeed {
    /// A file store with at least this access, change feed and reach, and a quota report if
    /// `quota` is `Reported`.
    pub fn new(access: Access, delta: Delta, scope: StorageScope, quota: QuotaReport) -> Self {
        Self {
            access,
            delta,
            scope,
            quota,
        }
    }
}

/// A provider photo library.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
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

impl PhotosNeed {
    /// A photo library with at least this much read, albums and change feed, and upload and
    /// video where those are `Present`.
    pub fn new(
        library_read: LibraryRead,
        upload: Offered,
        albums: Albums,
        video: Offered,
        delta: Delta,
    ) -> Self {
        Self {
            library_read,
            upload,
            albums,
            video,
            delta,
        }
    }
}

/// Small synced items.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct KeyValueNeed {
    /// At least this change feed.
    pub delta: Delta,
    /// Items at least this large.
    pub max_item: Bytes,
}

impl KeyValueNeed {
    /// Small synced items with at least this change feed, up to this large.
    pub fn new(delta: Delta, max_item: Bytes) -> Self {
        Self { delta, max_item }
    }
}

/// Any push channel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PushNeed {}

impl PushNeed {
    /// Any push channel.
    pub fn new() -> Self {
        Self {}
    }
}
