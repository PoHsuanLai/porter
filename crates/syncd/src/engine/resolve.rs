//! Settling a stored conflict, as the owning app chooses by the dataset's rule: the journal
//! row goes back to the state that finishes the chosen side, and the next cycle does it.

use super::{Engine, SyncError};
use crate::clock::Clock;
use crate::dataset::Dataset;
use crate::journal::Op;
use porter_sync::{BaseVersion, ItemState, JournalItem, RemoteSide, Replica, Resolution};

impl<R: Replica, D: Dataset, K: Clock> Engine<R, D, K> {
    /// Settles conflict `number`. `false` when there is no such conflict (already settled).
    ///
    /// `KeepRemote` has the next cycle fetch the replica's version over the local one (or
    /// discard the local copy when the replica deleted it). `KeepLocal` has it upload the local
    /// content based on the replica's current version (or create it again when the replica
    /// deleted it); a local deletion is found again by the scan and removed.
    pub fn resolve(&self, number: i64, how: Resolution) -> Result<bool, SyncError> {
        let Some(stored) = self
            .journal
            .conflicts()?
            .into_iter()
            .find(|c| c.number == Some(number))
        else {
            return Ok(false);
        };
        let rows = self.journal.items()?;
        let row = rows.iter().find(|row| row.local == stored.local);
        let mut ops = vec![Op::DropConflict(number)];
        if let Some(row) = row {
            let id = stored.conflict.item.clone();
            let (version, deleted) = match &stored.conflict.remote {
                RemoteSide::Changed(v) | RemoteSide::Exists(_, v) => (v.clone(), false),
                RemoteSide::Deleted(v) => (v.clone(), true),
            };
            let settled = match (how, deleted) {
                (Resolution::KeepRemote, false) => JournalItem {
                    remote: Some(id),
                    remote_version: Some(version.clone()),
                    base: BaseVersion::At(version),
                    state: ItemState::Fetching,
                    ..row.clone()
                },
                (Resolution::KeepRemote, true) => JournalItem {
                    remote_version: Some(version),
                    state: ItemState::Discarding,
                    ..row.clone()
                },
                (Resolution::KeepLocal, false) => JournalItem {
                    remote: Some(id),
                    remote_version: Some(version.clone()),
                    base: BaseVersion::At(version),
                    state: ItemState::PendingUpload,
                    ..row.clone()
                },
                (Resolution::KeepLocal, true) => JournalItem {
                    remote: None,
                    remote_version: None,
                    base: BaseVersion::Absent,
                    state: ItemState::PendingUpload,
                    ..row.clone()
                },
            };
            ops.push(Op::PutItem(settled));
        }
        self.journal.apply(&ops)?;
        Ok(true)
    }
}
