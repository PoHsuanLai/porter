//! The datasets syncd carries, each with its own conflict rule (design/31 §6.2). Mail,
//! calendars and contacts are not here: they sync by their protocols.

use serde::{Deserialize, Serialize};

/// A dataset syncd keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetKind {
    /// Photo originals: content-addressed, immutable.
    PhotosOriginals,
    /// Photo metadata and edit recipes: a manifest log.
    PhotosMetadata,
    /// Selectively synced files.
    Files,
    /// Settings, Spaces, dock, widgets and style CSS: an encrypted item log.
    Settings,
    /// Keychain items.
    Keychain,
}

/// How a dataset settles a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictRule {
    /// None can happen: the same hash is the same thing.
    Impossible,
    /// The later hybrid-logical-clock value wins, field by field.
    LastWriterPerField,
    /// Both are kept: "name (conflict from <device>)".
    KeepBoth,
    /// The conflict object is shown in the owning app.
    ShowInApp,
}

impl DatasetKind {
    /// The dataset's conflict rule.
    pub fn conflict_rule(self) -> ConflictRule {
        match self {
            DatasetKind::PhotosOriginals => ConflictRule::Impossible,
            DatasetKind::PhotosMetadata | DatasetKind::Settings => ConflictRule::LastWriterPerField,
            DatasetKind::Files => ConflictRule::KeepBoth,
            DatasetKind::Keychain => ConflictRule::ShowInApp,
        }
    }
}
