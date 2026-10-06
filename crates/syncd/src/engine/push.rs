//! The local side going out: the scan's changes recorded, then each upload and removal against
//! its base version.

use super::{Compared, Engine, Halt, Outcome, Report};
use crate::clock::Clock;
use crate::dataset::{Dataset, replica_hash};
use crate::journal::Op;
use porter_sync::{
    Acknowledgement, BaseVersion, Conflict, ItemState, JournalItem, LocalChange, PutItem,
    PutRefused, PutTarget, RemoteId, RemoteSide, RemoteVersion, Replica, TombstoneOrigin,
    local_changes,
};

impl<R: Replica, D: Dataset, K: Clock> Engine<R, D, K> {
    /// Records what the local side changed since the last cycle: the scan against the journal,
    /// as pending uploads and removals. It runs before the pull, so a local edit is never taken
    /// for an unchanged item the replica may overwrite.
    pub(super) async fn record_local(&self) -> Result<(), Halt> {
        let scan = self.dataset.scan().await?;
        let ops: Vec<Op> = local_changes(&self.journal.items()?, &scan)
            .into_iter()
            .map(|change| self.record(change))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        if !ops.is_empty() {
            self.journal.apply(&ops)?;
        }
        Ok(())
    }

    /// Sends the pending uploads and removals, each against its base version.
    pub(super) async fn push(&self, report: &mut Report) -> Result<(), Halt> {
        let mut rows = self.journal.items()?;
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        let mut full = false;
        for row in rows {
            match row.state {
                ItemState::PendingUpload if !full => {
                    match self.upload(&row, report).await {
                        // The rest of the uploads wait; removals still go.
                        Err(Halt::Stop(Outcome::QuotaFull)) => {
                            full = true;
                            report.outcome = Outcome::QuotaFull;
                        }
                        other => other?,
                    }
                }
                ItemState::PendingRemove => self.remove(&row, report).await?,
                _ => {}
            }
        }
        Ok(())
    }

    /// The journal ops that record one change the scan found.
    fn record(&self, change: LocalChange) -> Result<Vec<Op>, Halt> {
        let rows = self.journal.items()?;
        let held = |local: &porter_sync::LocalId| rows.iter().find(|row| row.local == *local);
        Ok(match change {
            LocalChange::New(seen) => vec![Op::PutItem(JournalItem {
                local: seen.local,
                remote: None,
                path: seen.path,
                size: seen.size,
                hash: Some(seen.hash),
                remote_version: None,
                base: BaseVersion::Absent,
                state: ItemState::PendingUpload,
            })],
            LocalChange::Changed(seen) => held(&seen.local)
                .map(|row| {
                    let base = match (&row.state, &row.remote_version) {
                        (ItemState::Synced, Some(version)) => BaseVersion::At(version.clone()),
                        _ => row.base.clone(),
                    };
                    Op::PutItem(JournalItem {
                        path: seen.path.clone(),
                        size: seen.size,
                        hash: Some(seen.hash.clone()),
                        base,
                        state: ItemState::PendingUpload,
                        ..row.clone()
                    })
                })
                .into_iter()
                .collect(),
            LocalChange::Removed(local) => held(&local)
                .map(|row| match row.remote {
                    // It never reached the replica: nothing to remove there.
                    None => Op::DeleteItem(local.clone()),
                    Some(_) => {
                        let base = match (&row.state, &row.remote_version) {
                            (ItemState::Synced, Some(version)) => BaseVersion::At(version.clone()),
                            _ => row.base.clone(),
                        };
                        Op::PutItem(JournalItem {
                            base,
                            state: ItemState::PendingRemove,
                            ..row.clone()
                        })
                    }
                })
                .into_iter()
                .collect(),
        })
    }

    async fn upload(&self, row: &JournalItem, report: &mut Report) -> Result<(), Halt> {
        // Gone since the scan: the next scan records the removal.
        let Ok(content) = self.dataset.read(&row.local).await else {
            report.refused += 1;
            return Ok(());
        };
        let (target, base) = match &row.remote {
            Some(id) => (PutTarget::Existing(id.clone()), row.base.clone()),
            None => (PutTarget::New(row.path.clone()), BaseVersion::Absent),
        };
        let hash = replica_hash(self.replica.features().hashes, &content.0);
        let put = PutItem {
            target,
            content,
            hash,
        };
        match self.replica.put(put, base).await {
            Ok((id, version)) => {
                self.journal
                    .apply(&[Op::PutItem(self.synced(row, &id, &version))])?;
                report.uploaded += 1;
                Ok(())
            }
            Err(PutRefused::Conflict(conflict)) => self.refused_upload(row, conflict, report).await,
            Err(PutRefused::Quota) => Err(Halt::Stop(Outcome::QuotaFull)),
            Err(PutRefused::Forbidden) => {
                report.refused += 1;
                Ok(())
            }
            Err(PutRefused::Transient(after)) => Err(Halt::Stop(Outcome::Retry(after))),
        }
    }

    /// A write whose base was stale: the same bytes already there are an adoption, anything
    /// else is a stored conflict.
    async fn refused_upload(
        &self,
        row: &JournalItem,
        conflict: Conflict,
        report: &mut Report,
    ) -> Result<(), Halt> {
        if let RemoteSide::Exists(id, version) = &conflict.remote
            && matches!(self.compare(row, id, None).await?, Compared::Same)
        {
            return Ok(self
                .journal
                .apply(&[Op::PutItem(self.synced(row, id, version))])?);
        }
        let ops = self.conflict_ops(row, conflict, report);
        Ok(self.journal.apply(&ops)?)
    }

    async fn remove(&self, row: &JournalItem, report: &mut Report) -> Result<(), Halt> {
        let Some(id) = &row.remote else {
            return Ok(self.journal.apply(&[Op::DeleteItem(row.local.clone())])?);
        };
        match self.replica.remove(id, row.base.clone()).await {
            Ok(version) => self.removed(row, id, &version, Acknowledgement::Pending, report),
            // Already deleted there: the goal is reached.
            Err(PutRefused::Conflict(Conflict {
                remote: RemoteSide::Deleted(version),
                ..
            })) => self.removed(row, id, &version, Acknowledgement::Acknowledged, report),
            Err(PutRefused::Conflict(conflict)) => {
                let ops = self.conflict_ops(row, conflict, report);
                Ok(self.journal.apply(&ops)?)
            }
            Err(PutRefused::Quota | PutRefused::Forbidden) => {
                report.refused += 1;
                Ok(())
            }
            Err(PutRefused::Transient(after)) => Err(Halt::Stop(Outcome::Retry(after))),
        }
    }

    fn removed(
        &self,
        row: &JournalItem,
        id: &RemoteId,
        version: &RemoteVersion,
        ack: Acknowledgement,
        report: &mut Report,
    ) -> Result<(), Halt> {
        self.journal.apply(&[
            Op::DeleteItem(row.local.clone()),
            self.tombstone(id, version, TombstoneOrigin::Local, ack),
        ])?;
        report.removed += 1;
        Ok(())
    }
}
