//! The vocabulary's version.

use serde::{Deserialize, Serialize};

/// The version of the whole vocabulary. Adding a kind or a field bumps it; the daemon and the
/// client crate release together, and an older client skips kinds it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VocabVersion(pub u16);

impl VocabVersion {
    /// The version this build speaks.
    pub const CURRENT: VocabVersion = VocabVersion(13);
}
