//! The journal's vocabulary (design/31 §6.1 "Journal"): what syncd keeps per dataset, as
//! values. The rows are here so the contract and its pure rules (`journal_reconcile`) stay
//! testable without a database; the SQLite file that holds them is syncd's.

use crate::anchor::Anchor;
use crate::change::Tombstone;
use crate::item::{BaseVersion, ContentHash, ItemPath, RemoteId, RemoteVersion};
use crate::refusal::Conflict;
use porter_core::{Bytes, UnixSeconds};
use serde::{Deserialize, Serialize};

/// The dataset's own id for an item it holds locally (a path, an asset id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocalId(pub String);

/// Where an item is between the local side and the replica. Every state names the one step
/// that finishes it, so a crash between two writes leaves a row the next run completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// Both sides agree on `remote_version`.
    Synced,
    /// The local side changed (or is new): upload against `base`.
    PendingUpload,
    /// The local side deleted it: remove against `base`.
    PendingRemove,
    /// The replica's content is being stored locally: fetch it again.
    Fetching,
    /// The replica deleted it: discard the local copy.
    Discarding,
    /// Both sides changed: a stored conflict waits for the owning app.
    Conflicted,
}

impl ItemState {
    /// Every state, for the slug table.
    pub const ALL: [ItemState; 6] = [
        ItemState::Synced,
        ItemState::PendingUpload,
        ItemState::PendingRemove,
        ItemState::Fetching,
        ItemState::Discarding,
        ItemState::Conflicted,
    ];

    /// The state's name in the journal file.
    pub fn slug(self) -> &'static str {
        match self {
            ItemState::Synced => "synced",
            ItemState::PendingUpload => "pending_upload",
            ItemState::PendingRemove => "pending_remove",
            ItemState::Fetching => "fetching",
            ItemState::Discarding => "discarding",
            ItemState::Conflicted => "conflicted",
        }
    }

    /// The state a slug names.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.slug() == slug)
    }

    /// Whether the item still has work to do (what `Status` counts as pending).
    pub fn is_pending(self) -> bool {
        !matches!(self, ItemState::Synced | ItemState::Conflicted)
    }
}

/// One row of `items`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalItem {
    /// The dataset's id for it.
    pub local: LocalId,
    /// The replica's id; absent until the first upload (or while a fetch has no local copy).
    pub remote: Option<RemoteId>,
    /// Where it is in the dataset's folder.
    pub path: ItemPath,
    /// Its size as last seen locally.
    pub size: Bytes,
    /// The dataset's fingerprint of the local content, as last seen.
    pub hash: Option<ContentHash>,
    /// The replica's version as last seen.
    pub remote_version: Option<RemoteVersion>,
    /// The version a pending write is based on.
    pub base: BaseVersion,
    /// Where it is.
    pub state: ItemState,
}

/// Who made a tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneOrigin {
    /// The replica reported the deletion.
    Remote,
    /// We deleted the item.
    Local,
}

/// Whether the replica has shown the deletion back to us.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Acknowledgement {
    /// Not yet: the tombstone is kept.
    Pending,
    /// Seen in the feed (or applied locally): the tombstone may be compacted.
    Acknowledged,
}

/// One row of `tombstones`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTombstone {
    /// The deletion.
    pub tombstone: Tombstone,
    /// Who made it.
    pub origin: TombstoneOrigin,
    /// Whether it may go.
    pub ack: Acknowledgement,
}

/// A conflict as the journal keeps it: `{item, base, local, remote}` (design/31 §6.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredConflict {
    /// The row's number, once stored.
    pub number: Option<i64>,
    /// `item`, `base` and the replica's side.
    pub conflict: Conflict,
    /// The local side: which item.
    pub local: LocalId,
    /// The local content's fingerprint when the conflict arose.
    pub local_hash: Option<ContentHash>,
    /// When it arose.
    pub at: UnixSeconds,
}

/// How a conflict is settled (the owning app chooses by the dataset's rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Upload the local content over the replica's current version.
    KeepLocal,
    /// Take the replica's version and drop the local change.
    KeepRemote,
}

/// Where a dataset's change feed stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAnchor {
    /// The position.
    pub anchor: Anchor,
    /// When it was stored.
    pub at: UnixSeconds,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_slug_reads_back_and_matches_serde() {
        for state in ItemState::ALL {
            assert_eq!(ItemState::from_slug(state.slug()), Some(state));
            let json = serde_json::to_value(state).expect("json");
            assert_eq!(json.as_str(), Some(state.slug()));
        }
        assert_eq!(ItemState::from_slug("nope"), None);
    }

    #[test]
    fn only_unfinished_work_is_pending() {
        const CASES: &[(ItemState, bool)] = &[
            (ItemState::Synced, false),
            (ItemState::Conflicted, false),
            (ItemState::PendingUpload, true),
            (ItemState::PendingRemove, true),
            (ItemState::Fetching, true),
            (ItemState::Discarding, true),
        ];
        for (state, pending) in CASES {
            assert_eq!(state.is_pending(), *pending, "{state:?}");
        }
    }
}
