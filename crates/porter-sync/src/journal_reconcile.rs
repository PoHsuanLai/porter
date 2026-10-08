//! The journal's pure rules: what a full listing changes (after `AnchorExpired`) and what the
//! local side changed since the last scan. Neither touches a replica or a file.

use crate::change::{Change, Tombstone};
use crate::item::{ContentHash, ItemPath, RemoteItem, RemoteVersion};
use crate::journal::{ItemState, JournalItem, LocalId};
use porter_core::{Bytes, UnixSeconds};
use std::collections::BTreeSet;

/// The changes a full listing implies against the journal, as the feed would have said them:
/// an upsert for every listed item the journal does not hold at that version, a tombstone for
/// every journal item the listing no longer has. An item whose version is the journal's is
/// not mentioned, so applying the result re-uploads and re-fetches nothing known.
pub fn reconcile(known: &[JournalItem], listing: &[RemoteItem], now: UnixSeconds) -> Vec<Change> {
    let at_version = |item: &RemoteItem| {
        known.iter().any(|row| {
            row.remote.as_ref() == Some(&item.id)
                && row.remote_version.as_ref() == Some(&item.version)
        })
    };
    let mut changes: Vec<Change> = listing
        .iter()
        .filter(|item| !at_version(item))
        .map(|item| Change::Upsert(item.clone()))
        .collect();
    let listed: BTreeSet<_> = listing.iter().map(|item| &item.id).collect();
    let mut gone: Vec<&JournalItem> = known
        .iter()
        .filter(|row| row.remote.as_ref().is_some_and(|id| !listed.contains(id)))
        .collect();
    gone.sort_by(|a, b| a.remote.cmp(&b.remote));
    changes.extend(gone.into_iter().filter_map(|row| {
        Some(Change::Tombstone(Tombstone {
            id: row.remote.clone()?,
            version: row
                .remote_version
                .clone()
                .unwrap_or_else(|| RemoteVersion(String::new())),
            deleted_at: now,
        }))
    }));
    changes
}

/// More than this many discards, and more than half of what is held, is too many to do without
/// the person saying so (the other bound is every item held, however few).
pub const MASS_DELETE_FLOOR: usize = 20;

/// A full listing that would discard a great deal of what the journal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MassDelete {
    /// Items the listing would discard locally.
    pub discard: usize,
    /// Items the journal holds that the replica has (or had) a copy of.
    pub held: usize,
}

/// Whether `changes` (what a full listing implies) would discard all of a non-empty journal, or
/// more than [`MASS_DELETE_FLOOR`] items and more than half of it. An empty or failed listing
/// from a server that is in trouble looks exactly like that, and acting on it would wipe the
/// local mirror, so the caller halts for confirmation instead.
pub fn mass_delete(known: &[JournalItem], changes: &[Change]) -> Option<MassDelete> {
    // Only settled items are discarded by a deletion; a pending one becomes a conflict instead.
    let settled = |row: &&JournalItem| matches!(row.state, ItemState::Synced | ItemState::Fetching);
    let held: BTreeSet<_> = known
        .iter()
        .filter(settled)
        .filter_map(|row| row.remote.as_ref())
        .collect();
    let discard = changes
        .iter()
        .filter(|change| matches!(change, Change::Tombstone(t) if held.contains(&t.id)))
        .count();
    let all = discard > 0 && discard == held.len();
    let most = discard > MASS_DELETE_FLOOR && discard * 2 > held.len();
    (all || most).then_some(MassDelete {
        discard,
        held: held.len(),
    })
}

/// What the local side holds now, one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanned {
    /// The dataset's id.
    pub local: LocalId,
    /// Its path.
    pub path: ItemPath,
    /// Its size.
    pub size: Bytes,
    /// The dataset's fingerprint of its content.
    pub hash: ContentHash,
}

/// What a scan says about one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalChange {
    /// Held locally, unknown to the journal.
    New(Scanned),
    /// Held locally with other content than the journal knows (a synced item, or a pending
    /// one that changed again), or held again after a pending removal.
    Changed(Scanned),
    /// Gone locally while the journal had it synced or waiting to upload.
    Removed(LocalId),
}

/// The changes a scan makes against the journal. Items mid-fetch, mid-discard or in conflict
/// are left to their own step.
pub fn local_changes(known: &[JournalItem], scan: &[Scanned]) -> Vec<LocalChange> {
    let mut out: Vec<LocalChange> = Vec::new();
    for seen in scan {
        match known.iter().find(|row| row.local == seen.local) {
            None => out.push(LocalChange::New(seen.clone())),
            Some(row) => {
                let same = row.hash.as_ref() == Some(&seen.hash) && row.size == seen.size;
                match (row.state, same) {
                    (ItemState::Synced | ItemState::PendingUpload, false)
                    | (ItemState::PendingRemove, _) => out.push(LocalChange::Changed(seen.clone())),
                    _ => {}
                }
            }
        }
    }
    let held: BTreeSet<&LocalId> = scan.iter().map(|seen| &seen.local).collect();
    out.extend(
        known
            .iter()
            .filter(|row| matches!(row.state, ItemState::Synced | ItemState::PendingUpload))
            .filter(|row| !held.contains(&row.local))
            .map(|row| LocalChange::Removed(row.local.clone())),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{BaseVersion, RemoteId};

    fn row(
        local: &str,
        remote: Option<&str>,
        version: Option<&str>,
        hash: &str,
        state: ItemState,
    ) -> JournalItem {
        JournalItem {
            local: LocalId(local.into()),
            remote: remote.map(|r| RemoteId(r.into())),
            path: ItemPath(local.into()),
            size: Bytes(1),
            hash: Some(ContentHash(hash.into())),
            remote_version: version.map(|v| RemoteVersion(v.into())),
            base: BaseVersion::Absent,
            state,
        }
    }

    fn listed(id: &str, version: &str) -> RemoteItem {
        RemoteItem {
            id: RemoteId(id.into()),
            version: RemoteVersion(version.into()),
            path: ItemPath(id.into()),
            size: Bytes(1),
            hash: None,
        }
    }

    fn scanned(local: &str, hash: &str) -> Scanned {
        Scanned {
            local: LocalId(local.into()),
            path: ItemPath(local.into()),
            size: Bytes(1),
            hash: ContentHash(hash.into()),
        }
    }

    #[test]
    fn a_listing_mentions_only_what_differs_from_the_journal() {
        let known = [
            row("a", Some("ra"), Some("v1"), "h", ItemState::Synced),
            row("b", Some("rb"), Some("v1"), "h", ItemState::Synced),
            row("c", Some("rc"), Some("v1"), "h", ItemState::PendingUpload),
            row("d", None, None, "h", ItemState::PendingUpload),
        ];
        let listing = [listed("ra", "v1"), listed("rb", "v2"), listed("rnew", "v1")];
        let changes = reconcile(&known, &listing, UnixSeconds(9));
        let said: Vec<String> = changes
            .iter()
            .map(|c| match c {
                Change::Upsert(item) => format!("up {} {}", item.id.0, item.version.0),
                Change::Tombstone(t) => {
                    format!("gone {} {} @{}", t.id.0, t.version.0, t.deleted_at.0)
                }
            })
            .collect();
        assert_eq!(said, ["up rb v2", "up rnew v1", "gone rc v1 @9"]);
    }

    #[test]
    fn a_listing_equal_to_the_journal_changes_nothing() {
        let known = [row("a", Some("ra"), Some("v1"), "h", ItemState::Synced)];
        assert!(reconcile(&known, &[listed("ra", "v1")], UnixSeconds(0)).is_empty());
        assert!(reconcile(&[], &[], UnixSeconds(0)).is_empty());
    }

    fn synced_rows(count: usize) -> Vec<JournalItem> {
        (0..count)
            .map(|n| {
                row(
                    &format!("l{n}"),
                    Some(&format!("r{n}")),
                    Some("v1"),
                    "h",
                    ItemState::Synced,
                )
            })
            .collect()
    }

    /// What listing only the first `kept` of `count` synced items implies.
    fn after_listing(count: usize, kept: usize) -> (Vec<JournalItem>, Vec<Change>) {
        let known = synced_rows(count);
        let listing: Vec<RemoteItem> = (0..kept).map(|n| listed(&format!("r{n}"), "v1")).collect();
        let changes = reconcile(&known, &listing, UnixSeconds(0));
        (known, changes)
    }

    #[test]
    fn a_listing_that_discards_everything_held_needs_confirmation_however_few_there_are() {
        for count in [1, 2, 5, 40] {
            let (known, changes) = after_listing(count, 0);
            assert_eq!(
                mass_delete(&known, &changes),
                Some(MassDelete {
                    discard: count,
                    held: count
                }),
                "{count}"
            );
        }
    }

    #[test]
    fn a_listing_that_discards_most_of_a_large_journal_needs_confirmation_and_less_does_not() {
        // (held, kept, needs confirmation)
        const CASES: &[(usize, usize, bool)] = &[
            // 21 of 40 is more than the floor and more than half.
            (40, 19, true),
            // Exactly the floor discarded (20 of 40): not more than the floor.
            (40, 20, false),
            // More than the floor but not more than half of a large journal.
            (100, 79, false),
            (100, 50, false),
            (100, 49, true),
            // Exactly half of a small one: not more than half.
            (42, 21, false),
            // A few of a small journal.
            (10, 7, false),
            (10, 1, false),
            // Nothing discarded.
            (50, 50, false),
            (0, 0, false),
        ];
        for (held, kept, want) in CASES {
            let (known, changes) = after_listing(*held, *kept);
            assert_eq!(
                mass_delete(&known, &changes).is_some(),
                *want,
                "{held} held, {kept} listed"
            );
        }
    }

    #[test]
    fn only_settled_items_count_towards_a_mass_delete() {
        let mut known = synced_rows(3);
        // Pending items the listing lacks become conflicts, not discards.
        known.push(row(
            "p",
            Some("rp"),
            Some("v1"),
            "h",
            ItemState::PendingUpload,
        ));
        known.push(row("n", None, None, "h", ItemState::PendingUpload));
        let changes = reconcile(&known, &[], UnixSeconds(0));
        assert_eq!(
            mass_delete(&known, &changes),
            Some(MassDelete {
                discard: 3,
                held: 3
            })
        );
        // A journal of only pending items has nothing to discard.
        let pending = [row(
            "p",
            Some("rp"),
            Some("v1"),
            "h",
            ItemState::PendingUpload,
        )];
        let changes = reconcile(&pending, &[], UnixSeconds(0));
        assert_eq!(mass_delete(&pending, &changes), None);
    }

    #[test]
    fn a_scan_against_the_journal() {
        struct Case {
            name: &'static str,
            known: Vec<JournalItem>,
            scan: Vec<Scanned>,
            want: Vec<LocalChange>,
        }
        let synced = |hash: &str| row("a", Some("ra"), Some("v1"), hash, ItemState::Synced);
        let pending = |state| row("a", Some("ra"), Some("v1"), "h", state);
        let cases = [
            Case {
                name: "unchanged",
                known: vec![synced("h")],
                scan: vec![scanned("a", "h")],
                want: vec![],
            },
            Case {
                name: "edited",
                known: vec![synced("h")],
                scan: vec![scanned("a", "h2")],
                want: vec![LocalChange::Changed(scanned("a", "h2"))],
            },
            Case {
                name: "new",
                known: vec![],
                scan: vec![scanned("a", "h")],
                want: vec![LocalChange::New(scanned("a", "h"))],
            },
            Case {
                name: "deleted",
                known: vec![synced("h")],
                scan: vec![],
                want: vec![LocalChange::Removed(LocalId("a".into()))],
            },
            Case {
                name: "pending upload edited again",
                known: vec![row("a", None, None, "h", ItemState::PendingUpload)],
                scan: vec![scanned("a", "h2")],
                want: vec![LocalChange::Changed(scanned("a", "h2"))],
            },
            Case {
                name: "pending upload deleted before it went up",
                known: vec![row("a", None, None, "h", ItemState::PendingUpload)],
                scan: vec![],
                want: vec![LocalChange::Removed(LocalId("a".into()))],
            },
            Case {
                name: "pending removal came back",
                known: vec![pending(ItemState::PendingRemove)],
                scan: vec![scanned("a", "h")],
                want: vec![LocalChange::Changed(scanned("a", "h"))],
            },
            Case {
                name: "mid-fetch rows are left alone",
                known: vec![pending(ItemState::Fetching)],
                scan: vec![],
                want: vec![],
            },
            Case {
                name: "conflicted rows are left alone",
                known: vec![pending(ItemState::Conflicted)],
                scan: vec![scanned("a", "other")],
                want: vec![],
            },
        ];
        for case in cases {
            assert_eq!(
                local_changes(&case.known, &case.scan),
                case.want,
                "{}",
                case.name
            );
        }
    }
}
