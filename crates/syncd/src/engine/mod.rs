//! The engine: one dataset against one replica (design/31 §6.1).
//!
//! A cycle is `resume` (finish what a crash left), `record_local` (the scan's changes become
//! pending rows, before anything is taken from the replica), `pull` (the replica's changes, paged, the
//! anchor stored only once a page is applied; a full listing reconciled by content hash when the
//! anchor expired), `push` (each pending write carrying its base version) and
//! a tombstone compaction. Every step of every operation is one journal transaction that leaves
//! a state naming the next step, so a crash between any two writes is finished by the next
//! cycle (`tests::cut_points`). A refusal of a write is a stored conflict, never an overwrite.

mod pull;
mod push;
mod resolve;
#[cfg(test)]
mod tests;

use crate::clock::Clock;
use crate::dataset::{Dataset, DatasetError, Direction, replica_hash};
use crate::journal::{Journal, JournalError, Op};
use porter_core::UnixSeconds;
use porter_core::capability::QuotaReport;
use porter_sync::{
    Acknowledgement, BaseVersion, Blob, ByteRange, Conflict, ItemState, JournalItem, LocalId,
    Quota, RemoteId, RemoteSide, RemoteVersion, Replica, ReplicaError, RetryAfter, StoredConflict,
    StoredTombstone, Tombstone, TombstoneOrigin,
};

/// Why a cycle stopped without finishing; the next cycle starts over from the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Outcome {
    /// Everything that could be done was.
    #[default]
    Done,
    /// The replica asked to be tried again later.
    Retry(RetryAfter),
    /// The account's token was refused: the account needs reauthentication.
    Unauthorized,
    /// The replica is full: uploads wait, the rest went on.
    QuotaFull,
    /// The dataset's folder is gone from the replica.
    Gone,
}

/// What one cycle did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// How it ended.
    pub outcome: Outcome,
    /// Items stored locally from the replica.
    pub fetched: usize,
    /// Items uploaded.
    pub uploaded: usize,
    /// Items removed from the replica.
    pub removed: usize,
    /// Items discarded locally because the replica deleted them.
    pub discarded: usize,
    /// Writes the replica refused as forbidden; they stay pending.
    pub refused: usize,
    /// Conflicts stored by this cycle.
    pub conflicts: Vec<StoredConflict>,
    /// The replica's quota, when it reports one.
    pub quota: Option<Quota>,
}

/// A cycle that could not run at all: the journal or the local side failed.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The journal could not be read or written.
    #[error(transparent)]
    Journal(#[from] JournalError),
    /// The local side failed.
    #[error(transparent)]
    Dataset(#[from] DatasetError),
}

/// Why a step stopped: the cycle ends with an outcome, or fails.
#[derive(Debug)]
pub(crate) enum Halt {
    Stop(Outcome),
    Fail(SyncError),
}

impl From<JournalError> for Halt {
    fn from(error: JournalError) -> Self {
        Halt::Fail(error.into())
    }
}

impl From<DatasetError> for Halt {
    fn from(error: DatasetError) -> Self {
        Halt::Fail(error.into())
    }
}

/// A replica read that failed, as the cycle's end.
pub(crate) fn halt(error: ReplicaError) -> Halt {
    Halt::Stop(match error {
        ReplicaError::Unauthorized => Outcome::Unauthorized,
        ReplicaError::Transient(after) => Outcome::Retry(after),
        ReplicaError::AnchorExpired | ReplicaError::Gone => Outcome::Gone,
    })
}

/// Whether the local copy and the replica's are the same bytes.
pub(crate) enum Compared {
    Same,
    /// They differ; the replica's bytes when they were fetched to find out.
    Differs(Option<Blob>),
}

/// One dataset syncing with one replica through one journal.
#[derive(Debug)]
pub struct Engine<R, D, K> {
    replica: R,
    dataset: D,
    journal: Journal,
    clock: K,
}

impl<R: Replica, D: Dataset, K: Clock> Engine<R, D, K> {
    /// An engine over `replica` and `dataset`, keeping its state in `journal`.
    pub fn new(replica: R, dataset: D, journal: Journal, clock: K) -> Self {
        Self {
            replica,
            dataset,
            journal,
            clock,
        }
    }

    /// The journal, for status and tests.
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The replica.
    pub fn replica(&self) -> &R {
        &self.replica
    }

    /// The local side.
    pub fn dataset(&self) -> &D {
        &self.dataset
    }

    /// Runs one cycle. A stop the replica asked for is the report's `outcome`; only a journal or
    /// local failure is an error.
    pub async fn sync_once(&self) -> Result<Report, SyncError> {
        let mut report = Report::default();
        match self.cycle(&mut report).await {
            Ok(()) => {}
            Err(Halt::Stop(outcome)) => report.outcome = outcome,
            Err(Halt::Fail(error)) => return Err(error),
        }
        Ok(report)
    }

    async fn cycle(&self, report: &mut Report) -> Result<(), Halt> {
        let direction = self.dataset.direction();
        self.resume(report).await?;
        // A pull-only dataset (a mirror) has no local changes to record and nothing to push.
        if direction == Direction::TwoWay {
            self.record_local().await?;
        }
        self.pull(report).await?;
        let pushed = match direction {
            Direction::TwoWay => self.push(report).await,
            Direction::PullOnly => Ok(()),
        };
        // Uploads that stopped for room still leave the cycle's other work and the quota read.
        if self.replica.features().quota == QuotaReport::Reported {
            report.quota = self.replica.quota().await.ok();
        }
        if self
            .journal
            .tombstones()?
            .iter()
            .any(|t| t.ack == Acknowledgement::Acknowledged)
        {
            self.journal.apply(&[Op::CompactTombstones])?;
        }
        pushed
    }

    /// The engine's clock now.
    pub(crate) fn now(&self) -> UnixSeconds {
        self.clock.now()
    }

    /// Whether the local copy of `row` is the replica's `remote` (by hash where the replica
    /// reports one this build can compute, else by fetching it).
    pub(crate) async fn compare(
        &self,
        row: &JournalItem,
        remote: &RemoteId,
        hash: Option<&porter_sync::ContentHash>,
    ) -> Result<Compared, Halt> {
        if matches!(row.state, ItemState::PendingRemove | ItemState::Fetching) {
            return Ok(Compared::Differs(None));
        }
        // A local copy that cannot be read is not known to match: never an adoption.
        let Ok(local) = self.dataset.read(&row.local).await else {
            return Ok(Compared::Differs(None));
        };
        let kind = self.replica.features().hashes;
        if let (Some(theirs), Some(mine)) = (hash, replica_hash(kind, &local.0)) {
            return Ok(if *theirs == mine {
                Compared::Same
            } else {
                Compared::Differs(None)
            });
        }
        match self.replica.fetch(remote, ByteRange::Whole).await {
            Ok(theirs) if theirs == local => Ok(Compared::Same),
            Ok(theirs) => Ok(Compared::Differs(Some(theirs))),
            Err(ReplicaError::Gone) => Ok(Compared::Differs(None)),
            Err(error) => Err(halt(error)),
        }
    }

    /// The ops that mark `row` conflicted and store the conflict, noting it in `report`.
    pub(crate) fn conflict_ops(
        &self,
        row: &JournalItem,
        conflict: Conflict,
        report: &mut Report,
    ) -> Vec<Op> {
        let stored = StoredConflict {
            number: None,
            conflict,
            local: row.local.clone(),
            local_hash: row.hash.clone(),
            at: self.now(),
        };
        report.conflicts.push(stored.clone());
        vec![
            Op::PutItem(JournalItem {
                state: ItemState::Conflicted,
                ..row.clone()
            }),
            Op::AddConflict(stored),
        ]
    }

    /// `row` as agreed with the replica at `version`.
    pub(crate) fn synced(
        &self,
        row: &JournalItem,
        remote: &RemoteId,
        version: &RemoteVersion,
    ) -> JournalItem {
        JournalItem {
            remote: Some(remote.clone()),
            remote_version: Some(version.clone()),
            base: BaseVersion::At(version.clone()),
            state: ItemState::Synced,
            ..row.clone()
        }
    }

    /// The tombstone ops for an item deleted at `version`.
    pub(crate) fn tombstone(
        &self,
        id: &RemoteId,
        version: &RemoteVersion,
        origin: TombstoneOrigin,
        ack: Acknowledgement,
    ) -> Op {
        Op::PutTombstone(StoredTombstone {
            tombstone: Tombstone {
                id: id.clone(),
                version: version.clone(),
                deleted_at: self.now(),
            },
            origin,
            ack,
        })
    }
}

/// The provisional local id of an item that is being fetched and has no local copy yet.
pub(crate) fn provisional(id: &RemoteId) -> LocalId {
    LocalId(format!("remote:{}", id.0))
}

/// The conflict `{item, base, remote}` of a write refused or a change that crossed one.
pub(crate) fn conflict_of(item: &RemoteId, base: BaseVersion, remote: RemoteSide) -> Conflict {
    Conflict {
        item: item.clone(),
        base,
        remote,
    }
}
