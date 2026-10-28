//! The drives shown in Files: each one's files on demand (`porter_drive::Drive` over its
//! journal), its folder in `~/Cloud`, and where it stands, shared between the storage supervisor
//! that runs them and `org.quire.Storage1` that serves them.
//!
//! A drive paused stays paused after a restart, and one kept downloaded whole stays kept (syncd's
//! settings keep both). An account being
//! removed lets its drive go first ([`DriveSet::retire`]): it is held (paused, its folder
//! taken away, and not started again until the removal ends), and what was made or changed in
//! it and not uploaded is written out as plain files to a folder of their own beside it,
//! `~/Cloud/<Drive> (Not Uploaded)`, which nothing removes. Only then are its cache and the
//! account removed, so a removal that fails, or is tried again, loses nothing and keeps
//! nothing twice. A drive not shown (Files turned off, or it did not start) has its cache
//! opened for this; a cache that cannot be read stops the removal.

use crate::paths::{AccountDir, Paths};
use porter_dbus::sync::DriveState;
use porter_drive::{Changed, Drive, DriveConfig, NodeId};
use porter_drive_fuse::Mounted;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

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

/// Whether a drive is let go while its account is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// Running.
    Live,
    /// Let go: paused, its folder taken away, not started again while the account is still here.
    Held,
    /// Let go, and its account stays (the removal did not go through): started again.
    Released,
}

/// What letting a drive go kept of what was not uploaded: how many files, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// How many files.
    pub count: usize,
    /// The folder they are in, beside the drive's (`~/Cloud/OneDrive (Not Uploaded)`).
    pub path: PathBuf,
}

/// The folder beside the drive's `folder` that what it did not upload is kept in.
fn not_uploaded(folder: &Path) -> PathBuf {
    let name = folder
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    folder.with_file_name(format!("{name} (Not Uploaded)"))
}

/// The bytes of a drive not shown: none can be had (only what is here is read).
struct Away;

impl porter_drive::Source for Away {
    fn fetch<'a>(
        &'a self,
        _id: &'a porter_drive::RemoteId,
        _start: u64,
        _len: u64,
    ) -> porter_drive::Fetching<'a> {
        Box::pin(std::future::ready(Err(porter_drive::SourceError::Offline)))
    }
}

/// Its folder as it is shown: being mounted, or mounted.
#[derive(Debug)]
enum Shown {
    Mounting(JoinHandle<Option<Mounted>>),
    Mounted(Mounted),
    /// A test's mount that is never taken away.
    #[cfg(test)]
    Stuck,
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
    /// Its folder being mounted, then mounted (dropping it takes the folder away).
    shown: Mutex<Option<Shown>>,
    hold: Mutex<Hold>,
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
            shown: Mutex::new(None),
            hold: Mutex::new(Hold::Live),
        }
    }

    /// Its folder, being mounted.
    pub fn set_mounting(&self, mounting: JoinHandle<Option<Mounted>>) {
        *held(&self.shown) = Some(Shown::Mounting(mounting));
    }

    /// Takes its folder away, waiting for a mount still being made so none is left behind (off
    /// the runtime's thread: it never waits for the session).
    ///
    /// # Errors
    /// The folder could not be taken away: it is still shown, and can be asked again.
    pub async fn take_away(&self) -> std::io::Result<()> {
        let shown = held(&self.shown).take();
        let mut mounted = match shown {
            None => return Ok(()),
            #[cfg(test)]
            Some(Shown::Stuck) => {
                *held(&self.shown) = Some(Shown::Stuck);
                return Err(std::io::Error::other("still mounted"));
            }
            Some(Shown::Mounted(mounted)) => mounted,
            Some(Shown::Mounting(mounting)) => match mounting.await {
                Ok(Some(mounted)) => mounted,
                _ => return Ok(()),
            },
        };
        let (left, taken) = tokio::task::spawn_blocking(move || {
            let taken = mounted.try_unmount();
            (taken.is_err().then_some(mounted), taken)
        })
        .await
        .map_err(std::io::Error::other)?;
        if let Some(mounted) = left {
            *held(&self.shown) = Some(Shown::Mounted(mounted));
        }
        taken
    }

    /// Shows its folder as a mount that is never taken away.
    #[cfg(test)]
    fn set_stuck(&self) {
        *held(&self.shown) = Some(Shown::Stuck);
    }

    /// Whether it was let go and its account stays: to be started again.
    pub fn released(&self) -> bool {
        *held(&self.hold) == Hold::Released
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

    /// Pauses or resumes reading its changes and sending what was changed here.
    pub fn set_paused(&self, paused: bool) {
        held(&self.standing).paused = paused;
        self.paused.send_replace(paused);
        self.drive.set_paused(paused);
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
    /// syncd's settings, which keep the drives paused, or kept downloaded whole, across
    /// restarts.
    settings: Option<PathBuf>,
    /// The accounts whose drives are held while they are removed.
    held: Arc<Mutex<std::collections::BTreeSet<String>>>,
    /// The drives whose room to keep everything downloaded changed (short, or no longer).
    room: broadcast::Sender<String>,
}

impl Default for DriveSet {
    fn default() -> Self {
        Self {
            drives: Arc::default(),
            events: broadcast::channel(1024).0,
            settings: None,
            held: Arc::default(),
            room: broadcast::channel(64).0,
        }
    }
}

impl DriveSet {
    /// The set, keeping which drives are paused in `file` (syncd's settings) across restarts.
    pub fn with_settings(mut self, file: PathBuf) -> Self {
        self.settings = Some(file);
        self
    }

    /// Pauses or resumes the drive `id`, and keeps that across restarts; whether it is here.
    pub fn set_paused(&self, id: &str, paused: bool) -> bool {
        let Some(drive) = self.get(id) else {
            return false;
        };
        drive.set_paused(paused);
        if let Some(file) = &self.settings
            && let Err(why) = crate::settings::write_paused(file, id, paused)
        {
            eprintln!("syncd: whether the drive is paused could not be saved: {why}");
        }
        true
    }

    /// Whether the drive `id` was left paused.
    pub fn was_paused(&self, id: &str) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|file| crate::settings::read_paused(file).contains(id))
    }

    /// Keeps the drive `id` downloaded whole, or no longer, from now on and after a restart: the
    /// drive shown now follows at once, one shown later when it starts ([`DriveSet::keeps_all`]).
    ///
    /// # Errors
    /// syncd's settings could not be written; nothing changed.
    pub fn set_keep_all(&self, id: &str, on: bool) -> std::io::Result<()> {
        if let Some(file) = &self.settings {
            crate::settings::write_keep_all(file, id, on)?;
        }
        if let Some(drive) = self.get(id) {
            drive.drive.set_keep_all(on);
        }
        Ok(())
    }

    /// Whether the drive `id` is kept downloaded whole, as syncd's settings say (also while it is
    /// not shown).
    pub fn keeps_all(&self, id: &str) -> bool {
        match &self.settings {
            Some(file) => crate::settings::read_keep_all(file).contains(id),
            None => self.get(id).is_some_and(|live| live.drive.keeps_all()),
        }
    }

    /// Whether keeping the drive `id` downloaded whole needs more room than this computer has
    /// free; `false` for a drive not shown.
    pub fn short_of_room(&self, id: &str) -> bool {
        self.get(id).is_some_and(|live| live.drive.short_of_room())
    }

    /// Lets the drive of `account` go before the account is removed: it is held (paused, its
    /// folder taken away, and not started again until [`DriveSet::release`] or
    /// [`DriveSet::forget`]), and what was not uploaded is written to the folder beside its own
    /// (see [`Kept`]). A drive not shown has its cache opened for this. What was kept; nothing
    /// when the account has no cache.
    ///
    /// # Errors
    /// The folder could not be taken away (nothing was written), the cache could not be read,
    /// or what was not uploaded could not be written out: the removal is not to go on.
    pub async fn retire(&self, paths: &Paths, account: &AccountDir) -> Result<Vec<Kept>, String> {
        held(&self.held).insert(account.as_str().to_owned());
        let cache = paths.drive_cache_dir(account);
        let (drive, path) = match self.get(account.as_str()) {
            Some(live) => {
                *held(&live.hold) = Hold::Held;
                live.set_paused(true);
                live.take_away().await.map_err(|why| why.to_string())?;
                (Some(live.drive.clone()), live.path.clone())
            }
            None if !cache.exists() => return Ok(Vec::new()),
            None => (None, super::shown_at(paths, account)),
        };
        let journal = super::journal_path(paths, account);
        let into = not_uploaded(&path);
        let to = into.clone();
        // Opened here, not in the blocking task: opening waits for the cache's lock on the
        // runtime, yielding to it.
        let drive = match drive {
            Some(drive) => drive,
            None => {
                let listing = super::DriveJournal::open(&journal).map_err(|e| e.to_string())?;
                Drive::open(DriveConfig::new(cache), Arc::new(listing), Arc::new(Away))
                    .await
                    .map_err(|e| e.to_string())?
            }
        };
        let count =
            tokio::task::spawn_blocking(move || drive.keep_unsent(&to).map_err(|e| e.to_string()))
                .await
                .map_err(|why| why.to_string())??;
        Ok(vec![Kept { count, path: into }])
    }

    /// Whether the drive of the account `id` is held for its removal: not to be started.
    pub fn is_held(&self, id: &str) -> bool {
        held(&self.held).contains(id)
    }

    /// The account `id` stays after all (its removal did not go through): its drive, let go,
    /// is started again.
    pub fn release(&self, id: &str) {
        held(&self.held).remove(id);
        if let Some(live) = self.get(id) {
            let mut hold = held(&live.hold);
            if *hold == Hold::Held {
                *hold = Hold::Released;
            }
        }
    }

    /// The account `id` is gone: no pause of its drive is kept, nor that it was kept whole.
    pub fn forget(&self, id: &str) {
        held(&self.held).remove(id);
        if let Some(file) = &self.settings
            && crate::settings::read_paused(file).contains(id)
            && let Err(why) = crate::settings::write_paused(file, id, false)
        {
            eprintln!("syncd: whether the drive is paused could not be saved: {why}");
        }
        if let Some(file) = &self.settings
            && crate::settings::read_keep_all(file).contains(id)
            && let Err(why) = crate::settings::write_keep_all(file, id, false)
        {
            eprintln!("syncd: whether to keep the drive downloaded could not be saved: {why}");
        }
    }

    /// Adds `drive`, telling of its items as they change from now on, and of its room to keep
    /// everything downloaded each time that changes ([`DriveSet::watch_room`]).
    pub fn insert(&self, drive: Arc<LiveDrive>) {
        let mut room = drive.drive.watch_room();
        let (told, id) = (self.room.clone(), drive.id.clone());
        let weak = Arc::downgrade(&drive);
        tokio::spawn(async move {
            // What it was when it came is told too: a drive started short of room says so.
            if *room.borrow_and_update() {
                let _ = told.send(id.clone());
            }
            while room.changed().await.is_ok() {
                if weak.upgrade().is_none() {
                    return;
                }
                room.borrow_and_update();
                let _ = told.send(id.clone());
            }
        });
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

    /// Hears the ids of the drives whose room to keep everything downloaded changes from now on:
    /// short of it, or no longer ([`porter_drive::Drive::short_of_room`]).
    pub fn watch_room(&self) -> broadcast::Receiver<String> {
        self.room.subscribe()
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
        let dir = test_kit::Scratch::new("drive-files");
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
        .await
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

    #[tokio::test]
    async fn keeping_a_drive_whole_outlasts_a_restart_and_goes_with_its_account() {
        let dir = test_kit::Scratch::new("drive-files-keep-all");
        let settings = dir.join("syncd.toml");
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        listing.file("a", None, "a.bin", 4, "1");
        source.put("a", b"aaaa".to_vec());
        let drive = Drive::open(
            DriveConfig::new(dir.join("cache")),
            listing.clone(),
            source.clone(),
        )
        .await
        .expect("drive");
        let live = Arc::new(LiveDrive::new(
            "one".into(),
            "/org/quire/Accounts1/account/one".into(),
            "ada@example.test".into(),
            "OneDrive",
            dir.join("Cloud").join("OneDrive"),
            drive.clone(),
        ));
        let set = DriveSet::default().with_settings(settings.clone());
        set.insert(Arc::clone(&live));
        set.set_keep_all("one", true).expect("saved");
        assert!(drive.keeps_all(), "the drive shown follows at once");
        // Not shown now: kept for when it is.
        set.set_keep_all("two", true).expect("saved");
        let again = DriveSet::default().with_settings(settings.clone());
        assert!(
            again.keeps_all("one") && again.keeps_all("two"),
            "after a restart"
        );
        set.set_keep_all("one", false).expect("saved");
        assert!(!drive.keeps_all());
        assert!(!again.keeps_all("one"));
        again.forget("two");
        assert!(!DriveSet::default().with_settings(settings).keeps_all("two"));
    }

    #[tokio::test]
    async fn a_pause_outlasts_a_restart_and_a_drive_let_go_writes_out_what_it_did_not_send() {
        let dir = test_kit::Scratch::new("drive-files-pause");
        let settings = dir.join("syncd.toml");
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        let sink = Arc::new(porter_drive::testing::MemorySink::new(
            listing.clone(),
            source.clone(),
            4,
        ));
        listing.file("k", None, "kept.txt", 1, "1");
        let drive = Drive::open_writable(
            DriveConfig::new(dir.join("cache")),
            listing.clone(),
            source.clone(),
            sink.clone(),
        )
        .await
        .expect("drive");
        let folder = dir.join("Cloud").join("OneDrive");
        let live = Arc::new(LiveDrive::new(
            "one".into(),
            "/org/quire/Accounts1/account/one".into(),
            "ada@example.test".into(),
            "OneDrive",
            folder.clone(),
            drive.clone(),
        ));
        let set = DriveSet::default().with_settings(settings.clone());
        set.insert(Arc::clone(&live));
        assert!(set.set_paused("one", true));
        assert!(!set.set_paused("two", true), "no such drive");
        let again = DriveSet::default().with_settings(settings.clone());
        assert!(again.was_paused("one"), "paused after a restart");
        assert!(!again.was_paused("two"));
        set.set_paused("one", false);
        assert!(
            !DriveSet::default()
                .with_settings(settings)
                .was_paused("one")
        );

        let (_, handle) = drive.create(&NodeId::Top, "new.txt").expect("create");
        drive.write(handle, 0, b"made here").await.expect("write");
        drive.release_write(handle);
        drive.set_paused(true);
        std::fs::create_dir_all(&folder).expect("folder");
        let paths = paths(&dir);
        let account = AccountDir::parse("one").expect("account");
        let kept_in = dir.join("Cloud").join("OneDrive (Not Uploaded)");
        let kept = set.retire(&paths, &account).await.expect("let go");
        assert_eq!(
            kept,
            [Kept {
                count: 1,
                path: kept_in.clone(),
            }]
        );
        assert_eq!(
            std::fs::read(kept_in.join("new.txt")).expect("kept"),
            b"made here"
        );
        assert_eq!(
            std::fs::read_dir(&folder).expect("folder").count(),
            0,
            "nothing under the drive's own folder, which a drive started again shows over"
        );
        assert!(live.paused().borrow().to_owned(), "nothing more is sent");
        assert!(set.is_held("one"), "not started again meanwhile");
        assert!(!live.released());
        // Tried again while the cache is still here: kept once.
        let kept = set.retire(&paths, &account).await.expect("let go");
        assert_eq!(kept[0].count, 1);
        // The removal did not go through: it starts again, and what was kept stays.
        set.release("one");
        assert!(live.released(), "started again when its account stays");
        assert!(!set.is_held("one"));
        let names: Vec<_> = std::fs::read_dir(&kept_in)
            .expect("folder")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["new.txt"]);
        // A crash after the cache was wiped, before the account went, and a removal again:
        // nothing to keep, and what was kept is there once.
        drop(set);
        drop(live);
        drop(drive);
        std::fs::remove_dir_all(dir.join("cache")).expect("wiped");
        let again = DriveSet::default();
        assert_eq!(again.retire(&paths, &account).await, Ok(Vec::new()));
        again.forget("one");
        let names: Vec<_> = std::fs::read_dir(&kept_in)
            .expect("folder")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["new.txt"]);
        let none = AccountDir::parse("none").expect("account");
        assert_eq!(
            DriveSet::default().retire(&paths, &none).await,
            Ok(Vec::new())
        );
    }

    #[tokio::test]
    async fn a_folder_that_cannot_be_taken_away_stops_the_removal_before_anything_is_kept() {
        let dir = test_kit::Scratch::new("drive-files-stuck");
        let listing = Arc::new(MemoryListing::default());
        let source = Arc::new(MemorySource::default());
        let sink = Arc::new(porter_drive::testing::MemorySink::new(
            listing.clone(),
            source.clone(),
            4,
        ));
        let drive =
            Drive::open_writable(DriveConfig::new(dir.join("cache")), listing, source, sink)
                .await
                .expect("drive");
        drive.set_paused(true);
        let (_, handle) = drive.create(&NodeId::Top, "new.txt").expect("create");
        drive.write(handle, 0, b"made here").await.expect("write");
        drive.release_write(handle);
        let folder = dir.join("Cloud").join("OneDrive");
        let live = Arc::new(LiveDrive::new(
            "one".into(),
            "/org/quire/Accounts1/account/one".into(),
            "ada@example.test".into(),
            "OneDrive",
            folder,
            drive,
        ));
        live.set_stuck();
        let set = DriveSet::default();
        set.insert(Arc::clone(&live));
        let paths = paths(&dir);
        let account = AccountDir::parse("one").expect("account");
        assert!(set.retire(&paths, &account).await.is_err());
        assert!(
            !dir.join("Cloud").join("OneDrive (Not Uploaded)").exists(),
            "nothing written"
        );
        assert!(
            live.take_away().await.is_err(),
            "asked again, it is still shown"
        );
    }

    fn paths(dir: &test_kit::Scratch) -> Paths {
        Paths {
            journals: dir.join("state/porter/sync"),
            mirrors: dir.join("data/porter/vdir"),
            callers_system: dir.join("none"),
            callers_user: dir.join("none"),
            cloud: dir.join("Cloud"),
            drive_cache: dir.join("cache/porter/drives"),
            settings: dir.join("none"),
        }
    }

    #[tokio::test]
    async fn what_a_drive_not_shown_did_not_send_is_kept_when_its_account_goes() {
        let dir = test_kit::Scratch::new("drive-files-not-shown");
        let paths = paths(&dir);
        let account = AccountDir::parse("one").expect("account");
        super::super::note_folder(&paths, &account, "OneDrive").expect("noted");
        let journal = Arc::new(
            super::super::DriveJournal::open(&super::super::journal_path(&paths, &account))
                .expect("journal"),
        );
        let source = Arc::new(MemorySource::default());
        let sink = Arc::new(porter_drive::testing::MemorySink::new(
            Arc::new(MemoryListing::default()),
            source.clone(),
            4,
        ));
        {
            // Changed while offline, then Files turned off: the drive is not shown any more.
            let drive = Drive::open_writable(
                DriveConfig::new(paths.drive_cache_dir(&account)),
                journal,
                source,
                sink,
            )
            .await
            .expect("drive");
            drive.set_paused(true);
            let (_, handle) = drive.create(&NodeId::Top, "offline.txt").expect("create");
            drive.write(handle, 0, b"not sent").await.expect("write");
            drive.release_write(handle);
        }
        let kept = DriveSet::default()
            .retire(&paths, &account)
            .await
            .expect("kept");
        let folder = dir.join("Cloud").join("OneDrive (Not Uploaded)");
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].count, &kept[0].path), (1, &folder));
        assert_eq!(
            std::fs::read(folder.join("offline.txt")).expect("kept"),
            b"not sent"
        );
    }
}
