//! Needs for the data kinds. Every field is a minimum in its enum's order.
//!
//! Each need grows by adding a minimum, so each is `#[non_exhaustive]` and built with `new`.

use porter_capability::{Access, Albums, Delta, LibraryRead, StorageScope};
use porter_ids::Bytes;
use serde::{Deserialize, Serialize};

/// An identity with at least these parts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct IdentityNeed {
    /// A display name and avatar.
    #[serde(with = "porter_capability::present")]
    pub profile: bool,
    /// A verified address.
    #[serde(with = "porter_capability::present")]
    pub verified_address: bool,
}

impl IdentityNeed {
    /// An identity with no part asked for.
    pub fn new() -> Self {
        Self {
            profile: false,
            verified_address: false,
        }
    }

    /// The same need, also asking for a display name and avatar.
    pub fn with_profile(mut self) -> Self {
        self.profile = true;
        self
    }

    /// The same need, also asking for a verified address.
    pub fn with_verified_address(mut self) -> Self {
        self.verified_address = true;
        self
    }
}

impl Default for IdentityNeed {
    fn default() -> Self {
        Self::new()
    }
}

/// A mailbox.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MailNeed {
    /// At least this access.
    pub access: Access,
    /// Sending, if set.
    #[serde(with = "porter_capability::present")]
    pub send: bool,
    /// At least this change feed.
    pub delta: Delta,
}

impl MailNeed {
    /// A mailbox with at least this access, sending if `send` is set, and this change feed.
    pub fn new(access: Access, send: bool, delta: Delta) -> Self {
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
    /// A quota report, if set.
    #[serde(with = "porter_capability::reported")]
    pub quota: bool,
}

impl StorageNeed {
    /// A file store with at least this access, change feed and reach, and a quota report if
    /// `quota` is set.
    pub fn new(access: Access, delta: Delta, scope: StorageScope, quota: bool) -> Self {
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
    /// Upload, if set.
    #[serde(with = "porter_capability::present")]
    pub upload: bool,
    /// At least these albums.
    pub albums: Albums,
    /// Video, if set.
    #[serde(with = "porter_capability::present")]
    pub video: bool,
    /// At least this change feed.
    pub delta: Delta,
}

impl PhotosNeed {
    /// A photo library with at least this much read, albums and change feed, and upload and
    /// video where those are set.
    pub fn new(
        library_read: LibraryRead,
        upload: bool,
        albums: Albums,
        video: bool,
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
