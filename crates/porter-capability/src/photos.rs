//! Photos (design/31 §2.2; R1).

use super::terms::{Delta, Offered};
use serde::{Deserialize, Serialize};

/// A provider with photo semantics. A Storage folder is a Photos library through the Photos
/// dataset, not through this kind (design/31 §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhotosCap {
    /// How much of the user's existing library can be read.
    pub library_read: LibraryRead,
    /// New items can be uploaded.
    pub upload: Offered,
    /// Which albums can be read and written.
    pub albums: Albums,
    /// Videos as well as stills.
    pub video: Offered,
    /// How changes become known.
    pub delta: Delta,
}

/// How much of an existing photo library an account reads. Ordered by reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryRead {
    /// Nothing.
    None,
    /// Only what the user picks in one provider-drawn session (Google Photos, C1).
    PickerOnly,
    /// The whole library.
    Full,
}

/// Which albums an account reaches. Ordered by reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Albums {
    /// None.
    None,
    /// Only albums this desktop created.
    AppCreated,
    /// Every album.
    Full,
}
