//! The drives shown in Files: each one's files on demand (`porter_drive::Drive` over its
//! journal), its folder in `~/Cloud`, and where it stands, shared between the storage supervisor
//! that runs them and `org.quire.Storage1` that serves them.

use porter_dbus::DriveState;
use porter_drive::{Changed, Drive, NodeId};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{broadcast, watch};

/// The cache limit every drive's eviction uses (Settings' `storage.cache_limit`), shared by the
/// settings module that sets it and the drives that follow it.
pub type CacheKnob = Arc<watch::Sender<porter_drive::CacheLimit>>;

/// What a drive needing sign-in says.
pub const SIGN_IN_WORDS: &str = "Sign in again to see this drive's files.";

/// What a drive whose listing cannot be read says.
pub const UNREADABLE_WORDS: &str = "This drive can't be read right now.";

fn held<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a drive stands, as its listing and its folder say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Standing {
    listed: DriveState,
    words: Option<String>,
    /// The folder could not be shown on this computer: what Files says.
    unshown: Option<String>,
    paused: bool,
}

/// One drive shown in Files.
#[derive(Debug)]
pub struct LiveDrive {
    /// Its id on the bus: the account's directory name, stable while the account is in syncd.
    pub id: String,
    /// The account's object path under `/org/quire/Accounts1/account/`.
    pub account: String,
    /// The account's label.
    pub label: String,
    /// The provider's name as people say it ("OneDrive", "Google Drive").
    pub provider: &'static str,
    /// Its folder (`~/Cloud/OneDrive`).
    pub path: PathBuf,
    /// Its files on demand.
    pub drive: Drive,
    standing: Mutex<Standing>,
    paused: watch::Sender<bool>,
}

impl LiveDrive {
    /// A drive, `starting` until its listing is read.
    pub fn new(
        id: String,
        account: String,
        label: String,
        provider: &'static str,
        path: PathBuf,
        drive: Drive,
    ) -> Self {
        Self {
            id,
            account,
            label,
            provider,
            path,
            drive,
            standing: Mutex::new(Standing {
                listed: DriveState::Starting,
                words: None,
                unshown: None,
                paused: false,
            }),
            paused: watch::channel(false).0,
        }
    }

    /// Its state and the words with it: a folder this computer cannot show first, then paused,
    /// then what the listing last said.
    pub fn state(&self) -> (DriveState, Option<String>) {
        let standing = held(&self.standing);
        if let Some(words) = &standing.unshown {
            return (DriveState::Error, Some(words.clone()));
        }
        if standing.paused {
            return (DriveState::Paused, None);
        }
        (standing.listed, standing.words.clone())
    }

    /// Notes what the listing said; whether the state changed.
    pub fn set_listed(&self, state: DriveState, words: Option<&str>) -> bool {
        let mut standing = held(&self.standing);
        let words = words.map(str::to_owned);
        let changed = standing.listed != state || standing.words != words;
        standing.listed = state;
        standing.words = words;
        changed
    }

    /// Notes that its folder could not be shown on this computer, and the words Files shows
    /// (porter-drive's `NO_FILES_HERE`, or porter-mountd's when it refused).
    pub fn set_unshown(&self, words: &str) {
        held(&self.standing).unshown = Some(words.to_owned());
    }

    /// Pauses or resumes reading its changes.
    pub fn set_paused(&self, paused: bool) {
        held(&self.standing).paused = paused;
        self.paused.send_replace(paused);
    }

    /// Whether it is paused, for its listing to wait on.
    pub fn paused(&self) -> watch::Receiver<bool> {
        self.paused.subscribe()
    }
}

/// What changed among the drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveEvent {
    /// A drive came or went, or its state, quota or bytes here changed.
    Drives,
    /// Items of one drive changed.
    Items {
        /// The drive's id.
        drive: String,
        /// The nodes whose details changed.
        ids: Vec<NodeId>,
    },
}

/// What the set tells of what `drive`'s changes said; `None` once they end. Changes missed
/// (the listener lagged) tell of every item the drive tracks again, so no item stays on a state
/// it left while nobody heard.
fn heard(
    drive: &LiveDrive,
    got: Result<Changed, broadcast::error::RecvError>,
) -> Option<DriveEvent> {
    let ids = match got {
        Ok(changed) => changed.0,
        Err(broadcast::error::RecvError::Lagged(_)) => drive.drive.tracked(),
        Err(broadcast::error::RecvError::Closed) => return None,
    };
    Some(DriveEvent::Items {
        drive: drive.id.clone(),
        ids,
    })
}

/// The drives shown in Files, by id. Clones share the set.
#[derive(Debug, Clone)]
pub struct DriveSet {
    drives: Arc<Mutex<BTreeMap<String, Arc<LiveDrive>>>>,
    events: broadcast::Sender<DriveEvent>,
}

impl Default for DriveSet {
    fn default() -> Self {
        Self {
            drives: Arc::default(),
            events: broadcast::channel(1024).0,
        }
    }
}

impl DriveSet {
    /// Adds `drive`, telling of its items as they change from now on.
    pub fn insert(&self, drive: Arc<LiveDrive>) {
        let mut changes = drive.drive.subscribe();
        let events = self.events.clone();
        let weak = Arc::downgrade(&drive);
        let id = drive.id.clone();
        held(&self.drives).insert(id.clone(), drive);
        tokio::spawn(async move {
            let mut local = None;
            loop {
                let got = changes.recv().await;
                let Some(drive) = weak.upgrade() else { return };
                let Some(items) = heard(&drive, got) else {
                    return;
                };
                let _ = events.send(items);
                let now = drive.drive.local_bytes();
                if local.replace(now) != Some(now) {
                    let _ = events.send(DriveEvent::Drives);
                }
            }
        });
        self.tell_drives();
    }

    /// Takes the drive `id` out.
    pub fn remove(&self, id: &str) -> Option<Arc<LiveDrive>> {
        let gone = held(&self.drives).remove(id);
        self.tell_drives();
        gone
    }

    /// Every drive, by id.
    pub fn all(&self) -> Vec<Arc<LiveDrive>> {
        held(&self.drives).values().cloned().collect()
    }

    /// The drive `id`.
    pub fn get(&self, id: &str) -> Option<Arc<LiveDrive>> {
        held(&self.drives).get(id).cloned()
    }

    /// Hears what changes from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<DriveEvent> {
        self.events.subscribe()
    }

    /// Tells that the drives changed.
    pub fn tell_drives(&self) {
        let _ = self.events.send(DriveEvent::Drives);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_drive::testing::{MemoryListing, MemorySource};
    use porter_drive::{DriveConfig, RemoteId};

    #[tokio::test]
    async fn changes_missed_tell_of_every_tracked_item_again() {
        let dir = porter_fake::Scratch::new("drive-files");
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        listing.file("a", None, "a.bin", 4, "1");
        source.put("a", b"aaaa".to_vec());
        source.hold();
        let drive = Drive::open(
            DriveConfig::new(dir.join("cache")),
            listing.clone(),
            source.clone(),
        )
        .expect("drive");
        let a = NodeId::Remote(RemoteId("a".into()));
        drive.open_file(&a).expect("open");
        let live = LiveDrive::new(
            "one".into(),
            "/org/quire/Accounts1/account/one".into(),
            "ada@example.test".into(),
            "OneDrive",
            dir.join("Cloud"),
            drive.clone(),
        );
        let told = heard(&live, Err(broadcast::error::RecvError::Lagged(3)));
        assert_eq!(
            told,
            Some(DriveEvent::Items {
                drive: "one".into(),
                ids: vec![a.clone()],
            }),
            "the download nobody heard of is told of again"
        );
        assert_eq!(heard(&live, Err(broadcast::error::RecvError::Closed)), None);
        source.release();
        drive.release(&a);
    }
}
