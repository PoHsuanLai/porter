//! The replica's side coming in: finishing interrupted fetches and discards, then the feed.

use super::{Compared, Engine, Halt, Outcome, Report, conflict_of, halt, provisional};
use crate::clock::Clock;
use crate::dataset::{Dataset, Direction, fingerprint};
use crate::journal::Op;
use porter_sync::{
    Acknowledgement, Anchor, BaseVersion, Blob, ByteRange, Change, Cursor, ItemState, JournalItem,
    More, RemoteId, RemoteItem, RemoteSide, Replica, ReplicaError, StoredAnchor, Tombstone,
    TombstoneOrigin, mass_delete, reconcile,
};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

impl<R: Replica, D: Dataset, K: Clock> Engine<R, D, K> {
    /// Finishes what a crash (or a transient stop) left half done.
    pub(super) async fn resume(&self, report: &mut Report) -> Result<(), Halt> {
        for row in self.journal.items()? {
            match row.state {
                ItemState::Fetching => {
                    if let Some(id) = row.remote.clone() {
                        self.fetch_into(&row, &id, None, report).await?;
                    }
                }
                ItemState::Discarding => self.finish_discard(&row, report).await?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads the feed (or the full listing) and applies it.
    pub(super) async fn pull(&self, report: &mut Report) -> Result<(), Halt> {
        let mut from = match self.journal.anchor()? {
            Some(stored) => Cursor::At(stored.anchor),
            None => Cursor::Start,
        };
        loop {
            match from {
                Cursor::At(ref anchor) => match self.replica.changes(from.clone()).await {
                    Ok(page) => {
                        for change in page.changes {
                            self.apply_change(change, report).await?;
                        }
                        self.set_anchor(page.next.clone())?;
                        match page.more {
                            More::More => from = Cursor::At(page.next),
                            More::Done => return Ok(()),
                        }
                    }
                    Err(ReplicaError::AnchorExpired) => {
                        let _ = anchor;
                        self.journal.apply(&[Op::ClearAnchor])?;
                        from = Cursor::Start;
                    }
                    Err(error) => return Err(halt(error)),
                },
                Cursor::Start => return self.relist(report).await,
            }
        }
    }

    fn set_anchor(&self, anchor: Anchor) -> Result<(), Halt> {
        let at = self.now();
        Ok(self
            .journal
            .apply(&[Op::SetAnchor(StoredAnchor { anchor, at })])?)
    }

    /// A full listing reconciled against the journal: nothing known is fetched again or
    /// uploaded, items the listing lacks are deletions, and the anchor is the listing's.
    async fn relist(&self, report: &mut Report) -> Result<(), Halt> {
        let mut all: Vec<Change> = Vec::new();
        let mut cursor = Cursor::Start;
        let last = loop {
            let page = self.replica.changes(cursor).await.map_err(halt)?;
            all.extend(page.changes);
            match page.more {
                More::More => cursor = Cursor::At(page.next),
                More::Done => break page.next,
            }
        };
        let listing: Vec<RemoteItem> = all
            .iter()
            .filter_map(|change| match change {
                Change::Upsert(item) => Some(item.clone()),
                Change::Tombstone(_) => None,
            })
            .collect();
        let rows = self.journal.items()?;
        let mut changes = reconcile(&rows, &listing, self.now());
        changes.extend(
            all.into_iter()
                .filter(|change| matches!(change, Change::Tombstone(_))),
        );
        let changes = self.rebind(&rows, changes).await?;
        // A listing that lacks all, or most, of what is held is more likely a server in trouble
        // (an empty answer, a half-built folder) than the person's wish: nothing is discarded
        // until they say so, and the next listing may show the items again. The anchor stays
        // cleared, so every cycle asks for the listing afresh.
        if let Some(mass) = mass_delete(&rows, &changes)
            && !self.mass_delete_confirmed.swap(false, Ordering::SeqCst)
        {
            return Err(Halt::Stop(Outcome::NeedsConfirmation(mass)));
        }
        for change in changes {
            self.apply_change(change, report).await?;
        }
        // The listing proves these are gone, as the feed would have said.
        let listed: BTreeSet<&RemoteId> = listing.iter().map(|item| &item.id).collect();
        let proven: Vec<Op> = self
            .journal
            .tombstones()?
            .into_iter()
            .filter(|t| t.ack == Acknowledgement::Pending && !listed.contains(&t.tombstone.id))
            .map(|t| Op::Acknowledge(t.tombstone.id))
            .collect();
        if !proven.is_empty() {
            self.journal.apply(&proven)?;
        }
        self.set_anchor(last)
    }

    /// An item the replica deleted and made again (a new id) with the content the journal holds
    /// at that path is the same item: the journal row moves to the new id, nothing is discarded
    /// or fetched. Those two changes are taken out of the list.
    async fn rebind(
        &self,
        rows: &[JournalItem],
        changes: Vec<Change>,
    ) -> Result<Vec<Change>, Halt> {
        let deleted: BTreeSet<RemoteId> = changes
            .iter()
            .filter_map(|change| match change {
                Change::Tombstone(t) => Some(t.id.clone()),
                Change::Upsert(_) => None,
            })
            .collect();
        let mut rebound: BTreeSet<RemoteId> = BTreeSet::new();
        for change in &changes {
            let Change::Upsert(item) = change else {
                continue;
            };
            let known = self.journal.item_by_remote(&item.id)?.is_some();
            let old = rows.iter().find(|row| {
                row.state == ItemState::Synced
                    && row.path == item.path
                    && row.remote.as_ref().is_some_and(|id| deleted.contains(id))
            });
            let (false, Some(old)) = (known, old) else {
                continue;
            };
            if matches!(
                self.compare(old, &item.id, item.hash.as_ref()).await?,
                Compared::Same
            ) {
                self.journal
                    .apply(&[Op::PutItem(self.synced(old, &item.id, &item.version))])?;
                rebound.extend(old.remote.clone());
                rebound.insert(item.id.clone());
            }
        }
        Ok(changes
            .into_iter()
            .filter(|change| match change {
                Change::Upsert(item) => !rebound.contains(&item.id),
                Change::Tombstone(t) => !rebound.contains(&t.id),
            })
            .collect())
    }

    async fn apply_change(&self, change: Change, report: &mut Report) -> Result<(), Halt> {
        match change {
            Change::Upsert(item) => self.upsert(item, report).await,
            Change::Tombstone(tombstone) => self.deleted(tombstone, report).await,
        }
    }

    async fn upsert(&self, item: RemoteItem, report: &mut Report) -> Result<(), Halt> {
        if let Some(row) = self.journal.item_by_remote(&item.id)? {
            return self.upsert_known(row, item, report).await;
        }
        match self.journal.new_item_at(&item.path)? {
            Some(row) => {
                // Ours, uploaded before a crash could record the id; or someone else's file at
                // the same path.
                let same = self.compare(&row, &item.id, item.hash.as_ref()).await?;
                let ops = match same {
                    Compared::Same => vec![Op::PutItem(self.synced(&row, &item.id, &item.version))],
                    Compared::Differs(_) => {
                        let remote = RemoteSide::Exists(item.id.clone(), item.version.clone());
                        let conflict = conflict_of(&item.id, BaseVersion::Absent, remote);
                        self.conflict_ops(&row, conflict, report)
                    }
                };
                Ok(self.journal.apply(&ops)?)
            }
            None => {
                let row = self.fetching_row(None, &item);
                self.fetch_into(&row, &item.id, None, report).await
            }
        }
    }

    async fn upsert_known(
        &self,
        row: JournalItem,
        item: RemoteItem,
        report: &mut Report,
    ) -> Result<(), Halt> {
        if row.remote_version.as_ref() == Some(&item.version) {
            return Ok(()); // our own write coming back
        }
        match row.state {
            ItemState::Conflicted | ItemState::Discarding => Ok(()),
            ItemState::Synced | ItemState::Fetching => {
                let compared = match row.state {
                    ItemState::Synced => self.compare(&row, &item.id, item.hash.as_ref()).await?,
                    _ => Compared::Differs(None),
                };
                match compared {
                    Compared::Same => Ok(self.journal.apply(&[Op::PutItem(self.synced(
                        &row,
                        &item.id,
                        &item.version,
                    ))])?),
                    Compared::Differs(fetched) => {
                        let edited = self.dataset.direction() == Direction::TwoWay
                            && self.moved_since_scan(&row).await;
                        if edited {
                            // Edited after this cycle's scan: not the replica's to overwrite.
                            let remote = RemoteSide::Changed(item.version.clone());
                            let conflict = conflict_of(&item.id, row.base.clone(), remote);
                            let ops = self.conflict_ops(&row, conflict, report);
                            return Ok(self.journal.apply(&ops)?);
                        }
                        let row = self.fetching_row(Some(&row), &item);
                        self.fetch_into(&row, &item.id, fetched, report).await
                    }
                }
            }
            ItemState::PendingUpload | ItemState::PendingRemove => {
                let ops = match self.compare(&row, &item.id, item.hash.as_ref()).await? {
                    Compared::Same => vec![Op::PutItem(self.synced(&row, &item.id, &item.version))],
                    Compared::Differs(_) => {
                        let remote = RemoteSide::Changed(item.version.clone());
                        let conflict = conflict_of(&item.id, row.base.clone(), remote);
                        self.conflict_ops(&row, conflict, report)
                    }
                };
                Ok(self.journal.apply(&ops)?)
            }
        }
    }

    /// Whether the local copy is no longer what the journal recorded (or cannot be read).
    async fn moved_since_scan(&self, row: &JournalItem) -> bool {
        match self.dataset.read(&row.local).await {
            Ok(local) => Some(fingerprint(&local.0)) != row.hash,
            Err(_) => true,
        }
    }

    /// The row that says "this item's replica content is being stored locally".
    fn fetching_row(&self, existing: Option<&JournalItem>, item: &RemoteItem) -> JournalItem {
        JournalItem {
            local: existing.map_or_else(|| provisional(&item.id), |row| row.local.clone()),
            remote: Some(item.id.clone()),
            path: item.path.clone(),
            size: item.size,
            hash: existing.and_then(|row| row.hash.clone()),
            remote_version: Some(item.version.clone()),
            base: BaseVersion::At(item.version.clone()),
            state: ItemState::Fetching,
        }
    }

    /// Records `row` as fetching, fetches (unless `fetched`), stores, and records it synced.
    async fn fetch_into(
        &self,
        row: &JournalItem,
        id: &RemoteId,
        fetched: Option<Blob>,
        report: &mut Report,
    ) -> Result<(), Halt> {
        if self
            .journal
            .item_by_remote(id)?
            .is_none_or(|held| held != *row)
        {
            self.journal.apply(&[Op::PutItem(row.clone())])?;
        }
        let content = match fetched {
            Some(blob) => blob,
            None => match self.replica.fetch(id, ByteRange::Whole).await {
                Ok(blob) => blob,
                // Deleted meanwhile: the feed's tombstone settles the row.
                Err(ReplicaError::Gone) => return Ok(()),
                Err(error) => return Err(halt(error)),
            },
        };
        let existing = (row.local != provisional(id)).then_some(&row.local);
        let stored = self.dataset.store(existing, &row.path, content).await?;
        let Some(version) = row.remote_version.clone() else {
            return Ok(());
        };
        let mut ops = vec![Op::PutItem(JournalItem {
            local: stored.local.clone(),
            size: stored.size,
            hash: Some(stored.hash),
            ..self.synced(row, id, &version)
        })];
        if stored.local != row.local {
            ops.insert(0, Op::DeleteItem(row.local.clone()));
        }
        if self.journal.tombstone_of(id)?.is_some() {
            ops.push(Op::Acknowledge(id.clone()));
        }
        self.journal.apply(&ops)?;
        report.fetched += 1;
        Ok(())
    }

    async fn deleted(&self, tombstone: Tombstone, report: &mut Report) -> Result<(), Halt> {
        let Some(row) = self.journal.item_by_remote(&tombstone.id)? else {
            let ours = self.journal.tombstone_of(&tombstone.id)?;
            if ours.is_some_and(|t| t.ack == Acknowledgement::Pending) {
                self.journal.apply(&[Op::Acknowledge(tombstone.id)])?;
            }
            return Ok(());
        };
        match row.state {
            ItemState::Synced | ItemState::Fetching | ItemState::Discarding => {
                let discarding = JournalItem {
                    state: ItemState::Discarding,
                    remote_version: Some(tombstone.version.clone()),
                    ..row
                };
                self.journal.apply(&[
                    Op::PutItem(discarding.clone()),
                    self.tombstone(
                        &tombstone.id,
                        &tombstone.version,
                        TombstoneOrigin::Remote,
                        Acknowledgement::Pending,
                    ),
                ])?;
                self.finish_discard(&discarding, report).await
            }
            ItemState::PendingUpload => {
                let remote = RemoteSide::Deleted(tombstone.version);
                let conflict = conflict_of(&tombstone.id, row.base.clone(), remote);
                let ops = self.conflict_ops(&row, conflict, report);
                Ok(self.journal.apply(&ops)?)
            }
            ItemState::PendingRemove => Ok(self.journal.apply(&[
                Op::DeleteItem(row.local),
                self.tombstone(
                    &tombstone.id,
                    &tombstone.version,
                    TombstoneOrigin::Remote,
                    Acknowledgement::Acknowledged,
                ),
            ])?),
            ItemState::Conflicted => Ok(()),
        }
    }

    async fn finish_discard(&self, row: &JournalItem, report: &mut Report) -> Result<(), Halt> {
        self.dataset.discard(&row.local).await?;
        let mut ops = vec![Op::DeleteItem(row.local.clone())];
        ops.extend(row.remote.iter().map(|id| Op::Acknowledge(id.clone())));
        self.journal.apply(&ops)?;
        report.discarded += 1;
        Ok(())
    }
}
