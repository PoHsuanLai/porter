//! One pass of a drive's listing: pages from where the journal stands until the feed is caught
//! up, each applied as it comes. An anchor that expired begins a full listing, which removes
//! what it no longer finds when it ends. Nothing is downloaded and nothing is written to the
//! drive; that is S2's, on demand.

use super::journal::DriveJournal;
use crate::journal::JournalError;
use porter_http::Http;
use porter_sync::{Cursor, Folders, More, Replica, ReplicaError, RetryAfter};
use std::sync::Arc;
use std::time::Duration;
use storage_gdrive::GdriveReplica;
use storage_graph::GraphReplica;
use tokio::sync::watch;

/// A replica of a whole drive whose tree the listing keeps between processes.
pub trait DriveReplica: Replica {
    /// The tree learnt so far, stored with each page.
    fn folders(&self) -> Folders;
}

impl<H: Http> DriveReplica for GraphReplica<H> {
    fn folders(&self) -> Folders {
        GraphReplica::folders(self)
    }
}

impl<H: Http> DriveReplica for GdriveReplica<H> {
    fn folders(&self) -> Folders {
        GdriveReplica::folders(self)
    }
}

/// How a pass ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Listed {
    /// Caught up.
    #[default]
    Done,
    /// The drive asked to be tried again later; what was applied stays.
    Retry(RetryAfter),
    /// The account has not given the access to the whole drive yet: nothing is listed until it
    /// is, and nothing asks for it again here.
    NeedsConsent,
    /// The account's sign-in was refused: it must sign in again.
    Unauthorized,
    /// The drive is gone (or its root could not be read).
    Gone,
}

/// What a pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pass {
    /// How it ended.
    pub outcome: Listed,
    /// Pages applied.
    pub pages: usize,
    /// Entries written.
    pub upserted: usize,
    /// Entries removed.
    pub removed: usize,
    /// Whether the anchor had expired, so the pass listed the drive again from the start.
    pub relisted: bool,
}

/// Reads `replica`'s pages into `journal` until the feed is caught up or stops. The replica
/// should start from the tree the journal kept (`with_folders(journal.folders()?)`).
///
/// # Errors
/// The journal could not be read or written; what was committed before stays.
pub async fn list_once<R: DriveReplica>(
    journal: &DriveJournal,
    replica: &R,
) -> Result<Pass, JournalError> {
    let mut pass = Pass::default();
    let mut cursor = journal.cursor()?;
    if cursor == Cursor::Start && !journal.listing()? {
        journal.begin_listing()?;
    }
    loop {
        let resuming = cursor != Cursor::Start;
        match replica.changes(cursor).await {
            Ok(page) => {
                let applied = journal.apply(&page, &replica.folders())?;
                pass.pages += 1;
                pass.upserted += applied.upserted;
                pass.removed += applied.removed;
                match page.more {
                    More::More => cursor = Cursor::At(page.next),
                    More::Done => return Ok(pass),
                }
            }
            // The feed dropped what the anchor stood for: list everything again, and remove
            // what that listing no longer has once it ends.
            Err(ReplicaError::AnchorExpired) if resuming && !pass.relisted => {
                journal.begin_listing()?;
                pass.relisted = true;
                cursor = Cursor::Start;
            }
            Err(error) => {
                pass.outcome = match error {
                    ReplicaError::NeedsConsent => Listed::NeedsConsent,
                    ReplicaError::Unauthorized => Listed::Unauthorized,
                    ReplicaError::Transient(after) => Listed::Retry(after),
                    ReplicaError::Gone => Listed::Gone,
                    // A listing from the start that the feed calls expired: try again later.
                    ReplicaError::AnchorExpired => Listed::Retry(RetryAfter(30)),
                    // a failure a later porter-sync adds: tried again later
                    _ => Listed::Retry(RetryAfter(30)),
                };
                return Ok(pass);
            }
        }
    }
}

/// Lists `replica` into `journal` now and then every `every`, for as long as the task lives. A
/// drive that asked to wait is listed again when it said; one whose access is missing, whose
/// sign-in was refused or that is gone is looked at again after `every`. The storage supervisor
/// ends the task when the grant goes; the journal stays.
pub async fn keep_listing<R: DriveReplica>(journal: DriveJournal, replica: R, every: Duration) {
    let (_running, paused) = watch::channel(false);
    let wanted = Arc::new(tokio::sync::Notify::new());
    keep_listing_told(Arc::new(journal), replica, every, paused, wanted, |_| {}).await;
}

/// [`keep_listing`] over a journal shared with the drive's files, telling `told` how each pass
/// ended. While `paused` says so no pass runs (a paused drive reads no changes); told by
/// `wanted` (the drive's files sent a change, or met a newer version), it lists at once.
pub async fn keep_listing_told<R: DriveReplica>(
    journal: Arc<DriveJournal>,
    replica: R,
    every: Duration,
    mut paused: watch::Receiver<bool>,
    wanted: Arc<tokio::sync::Notify>,
    mut told: impl FnMut(&Result<Pass, JournalError>) + Send,
) {
    loop {
        while *paused.borrow_and_update() {
            if paused.changed().await.is_err() {
                return;
            }
        }
        let pass = list_once(&journal, &replica).await;
        told(&pass);
        let wait = match pass {
            Ok(Pass {
                outcome: Listed::Retry(after),
                ..
            }) => Duration::from_secs(after.0.into()),
            Ok(_) => every,
            Err(why) => {
                eprintln!("syncd: a drive listing could not be kept: {why}");
                every
            }
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            // Resumed (or paused): look again now.
            changed = paused.changed() => if changed.is_err() { return },
            // A change was sent from here, or the drive has a newer version: look now.
            () = wanted.notified() => {}
        }
    }
}
