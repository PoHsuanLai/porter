//! The supervisor of Storage: every `rescan` it asks accountd which grants syncd holds (for each
//! [`StorageKind`]) and keeps one running mirror per granted account and kind.
//!
//! - `AppFolder` (`DataClass::Files`): the account's app folder as a two-way [`FolderDataset`]
//!   at `$XDG_DATA_HOME/porter/storage/<account>/`, one dataset `<account>/storage_app_folder`
//!   (owner `org.quire.Files`), over a [`graph_replica`] relay of the grant for a OneDrive
//!   account or a [`gdrive_replica`] relay for a Google one. Which one is read from the
//!   endpoint the account's candidate lists ([`app_folder_store`]), never from the provider.
//! - `Photos` (`DataClass::Photos`): the two Photos datasets over `Photos/Originals` and
//!   `Photos/Metadata` of the app folder of a Microsoft account. The grant is the switch: the
//!   account's Photos row in Settings makes it and takes it back.
//! - `GooglePhotos` (`DataClass::Photos`, a `Photos`-kind grant): Google Photos, by the same
//!   grant: the upload folder `$XDG_DATA_HOME/porter/photos/<account>/upload/` as the
//!   dataset `<account>/google_photos_upload` (owner `org.quire.Photos`), and the picker the
//!   Photos app imports through ([`StorageSupervisor::google_picker`]).
//! - `Drive` (`DataClass::Files`, a grant that reaches the whole store): the whole drive's
//!   listing into `<account>/storage_drive` ([`crate::datasets::drive::keep_listing`]), every
//!   [`StorageConfig::drive_every`]. It is no dataset of the hub: nothing syncs, it is listed.
//!
//! A new grant starts its mirror. A grant that is gone (revoked, or the account removed) stops
//! it: its datasets leave the hub (so `Sync1` stops listing them) and its engines end. What the
//! mirror kept is **not** deleted when only the grant goes: the files are the person's own, and
//! may hold edits that were never uploaded, so a new grant picks the folder up again; the
//! account's removal wipes it (`removal`). An accountd that cannot be asked changes nothing.

use super::folder::{FolderDataset, SLUG};
use super::grants::{
    AppFolderStore, StorageGrants, StorageKind, app_folder_store, google_photos_endpoints,
};
use crate::clock::Wall;
use crate::dataset::{Dataset, DatasetId};
use crate::datasets::drive::{
    self, CacheKnob, DriveJournal, DriveReplica, DriveSet, Listed, LiveDrive, SIGN_IN_WORDS,
    UNREADABLE_WORDS,
};
use crate::datasets::photos::google::{
    GooglePicker, PhotosApi, PhotosPicker, SLUG as UPLOAD_SLUG, UploadReplica, library_http,
    picker_http,
};
use crate::datasets::photos::{DeviceId, PhotoLibrary, PhotosRun, PhotosWiring};
use crate::datasets::pim::Wiring;
use crate::driver::Driver;
use crate::engine::Engine;
use crate::gdrive::gdrive_replica;
use crate::graph::graph_replica;
use crate::journal::Journal;
use crate::paths::AccountDir;
use crate::scheduler::next_look;
use crate::service::{Access, DatasetName};
use porter_client::GrantsLink;
use porter_core::{AppName, Candidate, EndpointUrl};
use porter_dbus::sync::DriveState;
use porter_drive::{CacheLimit, Drive, DriveConfig, FreeSpace, ReplicaSource};
use porter_drive_fuse::{DriveFs, Mounter, Now};
use porter_ids::WebUrl;
use porter_sync::{DriveWrite, Replica};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

/// The app that owns the app folder mirror's dataset (it settles its conflicts).
pub const FILES_APP: &str = "org.quire.Files";
/// The app that owns the Photos datasets.
pub const PHOTOS_APP: &str = "org.quire.Photos";

/// How the supervisor behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// How often grants are read again.
    pub rescan: Duration,
    /// Who owns the app folder dataset.
    pub files_owners: Access,
    /// Who owns the Photos datasets.
    pub photos_owners: Access,
    /// The title of the album Google Photos uploads go into (an album the app creates).
    pub google_album: String,
    /// How often a whole drive's listing reads its changes.
    pub drive_every: Duration,
    /// Whether a drive is shown as a folder at `~/Cloud/<Drive>`.
    pub mount: DriveMount,
    /// The cache limit the drives start with (Settings' `storage.cache_limit` changes it).
    pub cache_limit: CacheLimit,
    /// How each drive finds the free space keeping it downloaded whole is held against.
    pub free_space: FreeSpace,
}

/// How a drive shown in Files is put at its path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DriveMount {
    /// Not at all: its files are served on the bus only, and nothing is mounted (what a library
    /// user and the tests get unless they ask).
    #[default]
    Off,
    /// Mounted through FUSE as this mounter says (what the daemon does). A computer that cannot
    /// mount it reads the drive `error` with plain words.
    Fuse(Mounter),
}

fn owned_by(app: &str) -> Access {
    Access {
        owners: AppName::parse(app).into_iter().collect(),
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            rescan: Duration::from_secs(600),
            files_owners: owned_by(FILES_APP),
            photos_owners: owned_by(PHOTOS_APP),
            google_album: crate::datasets::photos::google::ALBUM_TITLE.to_owned(),
            drive_every: Duration::from_secs(300),
            mount: DriveMount::Off,
            cache_limit: CacheLimit::Automatic,
            free_space: FreeSpace::Measured,
        }
    }
}

enum Running<T: GrantsLink> {
    /// The app folder mirror, over either store.
    Folder {
        name: DatasetName,
        task: JoinHandle<()>,
    },
    Photos {
        names: Vec<DatasetName>,
        // Dropping it ends both engines.
        run: PhotosRun,
    },
    /// Google Photos: the upload engine when the account granted the append-only scope, and the
    /// picker when it granted the picker scope (each stands alone).
    GooglePhotos {
        upload: Option<(DatasetName, JoinHandle<()>)>,
        picker: Option<Arc<GooglePicker<T>>>,
    },
    /// The whole drive: its listing, its cache limit's follower, and its folder.
    Drive {
        task: JoinHandle<()>,
        limit: JoinHandle<()>,
        id: String,
        path: std::path::PathBuf,
    },
}

impl<T: GrantsLink> std::fmt::Debug for Running<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

impl<T: GrantsLink> Running<T> {
    fn names(&self) -> Vec<DatasetName> {
        match self {
            Running::Folder { name, .. } => vec![name.clone()],
            Running::GooglePhotos { upload, .. } => {
                upload.iter().map(|(name, _)| name.clone()).collect()
            }
            Running::Photos { names, .. } => names.clone(),
            Running::Drive { .. } => Vec::new(),
        }
    }
}

/// Keeps the Storage mirrors of every granted account.
#[derive(Debug)]
pub struct StorageSupervisor<T: GrantsLink, G> {
    wiring: Wiring<T>,
    grants: G,
    config: StorageConfig,
    running: BTreeMap<(AccountDir, StorageKind), Running<T>>,
    /// The Google Photos pickers running now, for the bus object that serves them.
    pickers: crate::service::Pickers<T>,
    silent: BTreeSet<StorageKind>,
    drives: DriveSet,
    limit: CacheKnob,
}

/// A device name for this machine's Photos library, made once (the library keeps it): the
/// account and the run's `nonce`, which differs between runs, so a library made by another run
/// (or a reinstall) is another device.
fn new_device(account: &AccountDir, nonce: &str) -> DeviceId {
    let digest = Sha256::digest(format!("{account}/{nonce}"));
    DeviceId::parse(&format!("d{}", hex::encode(&digest[..6]))).expect("a device name")
}

fn seed_of(text: &str) -> u64 {
    u64::from_be_bytes(
        Sha256::digest(text.as_bytes())[..8]
            .try_into()
            .unwrap_or([0; 8]),
    )
}

impl<T, G> StorageSupervisor<T, G>
where
    T: GrantsLink + 'static,
    G: StorageGrants + 'static,
{
    /// A supervisor over `wiring` (its `owners` are not used: `config` names the owners of each
    /// kind), asking `grants`.
    pub fn new(wiring: Wiring<T>, grants: G, config: StorageConfig) -> Self {
        let drives = DriveSet::default().with_settings(wiring.paths.settings.clone());
        Self {
            wiring,
            grants,
            running: BTreeMap::new(),
            pickers: crate::service::Pickers::default(),
            silent: BTreeSet::new(),
            drives,
            limit: Arc::new(watch::channel(config.cache_limit).0),
            config,
        }
    }

    /// The drives shown in Files, as `org.quire.Storage1` serves them: kept current by every
    /// `tick`.
    pub fn drive_set(&self) -> DriveSet {
        self.drives.clone()
    }

    /// The cache limit the drives follow, for Settings to set.
    pub fn cache_knob(&self) -> CacheKnob {
        Arc::clone(&self.limit)
    }

    /// The pickers as the bus object reads them: kept current by every `tick`.
    pub fn pickers(&self) -> crate::service::Pickers<T> {
        self.pickers.clone()
    }

    /// The Photos library of `account` while its Photos datasets run: what the Photos app will
    /// import into and read from (and a test's way to put a photo in).
    pub fn photos_library(&self, account: &AccountDir) -> Option<&PhotoLibrary> {
        match self.running.get(&(account.clone(), StorageKind::Photos))? {
            Running::Photos { run, .. } => Some(run.library()),
            Running::Folder { .. } | Running::GooglePhotos { .. } | Running::Drive { .. } => None,
        }
    }

    /// The Google Photos picker of `account` while its Google Photos run: what the Photos app
    /// calls to start a session, poll it and import what the person picked. `None` for an
    /// account that is not running Google Photos, or that has no picker endpoint.
    pub fn google_picker(&self, account: &AccountDir) -> Option<Arc<GooglePicker<T>>> {
        match self
            .running
            .get(&(account.clone(), StorageKind::GooglePhotos))?
        {
            Running::GooglePhotos { picker, .. } => picker.clone(),
            Running::Folder { .. } | Running::Photos { .. } | Running::Drive { .. } => None,
        }
    }

    /// The datasets running now.
    pub fn datasets(&self) -> Vec<DatasetName> {
        self.running.values().flat_map(Running::names).collect()
    }

    /// The accounts whose whole drive is being listed now.
    pub fn drives(&self) -> Vec<AccountDir> {
        self.running
            .keys()
            .filter(|(_, kind)| *kind == StorageKind::Drive)
            .map(|(account, _)| account.clone())
            .collect()
    }

    /// One look: grants, what to start and what to stop. `true` when every step went through;
    /// `false` when accountd could not be asked or a mirror could not be started (the next
    /// look is then soon, see [`next_look`]).
    pub async fn tick(&mut self) -> bool {
        let mut whole = true;
        for kind in StorageKind::ALL {
            let Ok(candidates) = self.grants.granted(kind).await else {
                eprintln!("syncd: cannot ask for the {kind:?} grants; asking again soon");
                whole = false;
                continue;
            };
            if candidates.is_empty() && self.silent.insert(kind) {
                eprintln!(
                    "syncd: no {kind:?} grant on an account it mirrors, so no {kind:?} is mirrored"
                );
            }
            let mut granted: Vec<AccountDir> = Vec::new();
            let folders = drive_folders(&candidates);
            for candidate in &candidates {
                let Some(account) =
                    AccountDir::parse(&porter_ids::object_segment(candidate.account.as_str()))
                else {
                    continue;
                };
                granted.push(account.clone());
                let key = (account.clone(), kind);
                // Held while its account is removed: neither started nor shown again.
                if !may_run(&self.drives, kind, &account) {
                    continue;
                }
                let folder = folders.get(&candidate.account).cloned().unwrap_or_default();
                // A drive whose folder name changed (a second account of its provider came or
                // went) is put at its new path; one let go for a removal that did not go
                // through is started again.
                if let Some(Running::Drive { path, id, .. }) = self.running.get(&key)
                    && (*path != self.wiring.paths.cloud.join(&folder)
                        || self.drives.get(id).is_some_and(|live| live.released()))
                    && let Some(running) = self.running.remove(&key)
                {
                    self.retire(running).await;
                }
                if self.running.contains_key(&key) {
                    continue;
                }
                match self.start(kind, &account, candidate, &folder).await {
                    Ok(running) => {
                        self.running.insert(key, running);
                    }
                    Err(why) => {
                        eprintln!(
                            "syncd: cannot mirror {kind:?} of {account}: {why}; trying again soon"
                        );
                        whole = false;
                    }
                }
            }
            let stale: Vec<(AccountDir, StorageKind)> = self
                .running
                .keys()
                .filter(|(account, k)| *k == kind && !granted.contains(account))
                .cloned()
                .collect();
            for key in stale {
                if let Some(running) = self.running.remove(&key) {
                    self.retire(running).await;
                }
            }
        }
        self.pickers.set(
            self.running
                .iter()
                .filter_map(|((account, _), running)| match running {
                    Running::GooglePhotos {
                        picker: Some(picker),
                        ..
                    } => Some((account.clone(), Arc::clone(picker))),
                    _ => None,
                })
                .collect(),
        );
        whole
    }

    async fn start(
        &self,
        kind: StorageKind,
        account: &AccountDir,
        candidate: &Candidate,
        folder: &str,
    ) -> Result<Running<T>, String> {
        let wiring = &self.wiring;
        let grant = || candidate.grant.clone();
        match kind {
            StorageKind::AppFolder => {
                let replica_of = |store: AppFolderStore<'_>| match store {
                    AppFolderStore::Graph(e) => graph_replica(
                        Arc::clone(&wiring.accounts),
                        grant(),
                        e.url.clone(),
                        "",
                        wiring.clock.clone(),
                    )
                    .map(Either::Left),
                    AppFolderStore::Drive(e) => gdrive_replica(
                        Arc::clone(&wiring.accounts),
                        grant(),
                        e.url.clone(),
                        "",
                        wiring.clock.clone(),
                    )
                    .map(Either::Right),
                };
                let store = app_folder_store(candidate).ok_or("no Graph or Drive endpoint")?;
                match replica_of(store).map_err(|e| e.to_string())? {
                    Either::Left(replica) => self.start_folder(account, replica),
                    Either::Right(replica) => self.start_folder(account, replica),
                }
            }
            StorageKind::Photos => {
                let endpoint =
                    super::grants::graph_endpoint(candidate).ok_or("no Graph endpoint")?;
                self.start_photos(account, candidate, &endpoint.url)
            }
            StorageKind::GooglePhotos => self.start_google_photos(account, candidate),
            StorageKind::Drive => self.start_drive(account, candidate, folder).await,
        }
    }

    /// The whole drive of `account`, over OneDrive's root or My Drive by the endpoint: its
    /// listing from the tree its journal kept, its files on demand over the journal and a
    /// replica of its own for fetching, and its folder `~/Cloud/<folder>`.
    async fn start_drive(
        &self,
        account: &AccountDir,
        candidate: &Candidate,
        folder: &str,
    ) -> Result<Running<T>, String> {
        let wiring = &self.wiring;
        let store = app_folder_store(candidate).ok_or("no Graph or Drive endpoint")?;
        match store {
            AppFolderStore::Graph(e) => {
                let replica = || {
                    graph_replica(
                        Arc::clone(&wiring.accounts),
                        candidate.grant.clone(),
                        e.url.clone(),
                        "",
                        wiring.clock.clone(),
                    )
                    .map(|r| r.with_root(storage_graph::Root::Drive))
                    .map_err(|e| e.to_string())
                };
                self.run_drive(
                    account,
                    candidate,
                    folder,
                    "OneDrive",
                    replica()?,
                    replica()?,
                )
                .await
            }
            AppFolderStore::Drive(e) => {
                let replica = || {
                    gdrive_replica(
                        Arc::clone(&wiring.accounts),
                        candidate.grant.clone(),
                        e.url.clone(),
                        "",
                        wiring.clock.clone(),
                    )
                    .map(|r| r.with_root(storage_gdrive::Root::Drive))
                    .map_err(|e| e.to_string())
                };
                self.run_drive(
                    account,
                    candidate,
                    folder,
                    "Google Drive",
                    replica()?,
                    replica()?,
                )
                .await
            }
        }
    }

    async fn run_drive<L, F>(
        &self,
        account: &AccountDir,
        candidate: &Candidate,
        folder: &str,
        provider: &'static str,
        listing: L,
        fetch: F,
    ) -> Result<Running<T>, String>
    where
        L: DriveReplica + WithFolders + 'static,
        F: Replica + DriveWrite + 'static,
    {
        let wiring = &self.wiring;
        let journal = Arc::new(
            DriveJournal::open(&drive::journal_path(&wiring.paths, account))
                .map_err(|e| e.to_string())?,
        );
        let listing = listing.with_folders(journal.folders().map_err(|e| e.to_string())?);
        let fetch = Arc::new(fetch);
        // What is changed in the folder is kept in the cache first, then sent through the same
        // replica the bytes come from.
        // Read before the open's await: the watch's guard is not held across it.
        let limit = *self.limit.borrow();
        let files = Drive::open_writable(
            DriveConfig {
                limit,
                free_space: self.config.free_space,
                ..DriveConfig::new(wiring.paths.drive_cache_dir(account))
            },
            Arc::clone(&journal) as Arc<dyn porter_drive::Listing>,
            Arc::new(ReplicaSource(Arc::clone(&fetch))),
            Arc::new(ReplicaSource(Arc::clone(&fetch))) as Arc<dyn porter_drive::Sink>,
        )
        .await
        .map_err(|e| e.to_string())?;
        let path = wiring.paths.cloud.join(folder);
        // Where what was not uploaded is kept if the account is removed while it is not shown.
        if let Err(why) = drive::note_folder(&wiring.paths, account, folder) {
            eprintln!("syncd: the folder of {account}'s drive was not noted: {why}");
        }
        let live = Arc::new(LiveDrive::new(
            account.as_str().to_owned(),
            porter_dbus::accounts::account_path(&candidate.account),
            candidate.label.0.clone(),
            provider,
            path.clone(),
            files.clone(),
        ));
        // Paused before a restart: paused still, before anything is read or sent.
        if self.drives.was_paused(&live.id) {
            live.set_paused(true);
        }
        // Kept downloaded whole before: kept so again, before its listing is read or anything
        // is evicted.
        if self.drives.keeps_all(&live.id) {
            files.set_keep_all(true);
        }
        // The folder is mounted off the runtime's one thread: asking porter-mountd, or running
        // fusermount3, waits on another process (bounded, but not for the runtime to sit on).
        let mounting = match &self.config.mount {
            DriveMount::Off => None,
            DriveMount::Fuse(mounter) => {
                let fs = DriveFs::new(files.clone(), tokio::runtime::Handle::current(), {
                    let millis = Arc::clone(&self.wiring.millis);
                    Now::new(move || std::time::UNIX_EPOCH + Duration::from_millis(millis.now_ms()))
                });
                let (mounter, path, live, set) = (
                    mounter.clone(),
                    path.clone(),
                    Arc::clone(&live),
                    self.drives.clone(),
                );
                let account = account.to_string();
                Some(tokio::task::spawn_blocking(move || {
                    match mounter.mount(fs, &path) {
                        Ok(mounted) => Some(mounted),
                        Err(why) => {
                            eprintln!("syncd: {account} is not shown at {}: {why}", path.display());
                            live.set_unshown(crate::words::unshown(&why));
                            set.tell_drives();
                            None
                        }
                    }
                }))
            }
        };
        if let Some(mounting) = mounting {
            live.set_mounting(mounting);
        }
        self.drives.insert(Arc::clone(&live));
        let mut knob = self.limit.subscribe();
        let follower = files.clone();
        let limit = tokio::spawn(async move {
            while knob.changed().await.is_ok() {
                let limit = *knob.borrow_and_update();
                follower.set_limit(limit);
            }
        });
        let set = self.drives.clone();
        let weak = Arc::downgrade(&live);
        let told = move |pass: &Result<drive::Pass, crate::journal::JournalError>| {
            let Some(live) = weak.upgrade() else { return };
            let (state, words) = match pass {
                Ok(pass) => match pass.outcome {
                    Listed::Done => (DriveState::Ready, None),
                    Listed::Retry(_) => (DriveState::Offline, None),
                    Listed::NeedsConsent | Listed::Unauthorized => {
                        (DriveState::NeedsSignIn, Some(SIGN_IN_WORDS))
                    }
                    Listed::Gone => (DriveState::Error, Some(UNREADABLE_WORDS)),
                },
                Err(_) => (DriveState::Error, Some(UNREADABLE_WORDS)),
            };
            live.drive.set_offline(state == DriveState::Offline);
            live.drive.set_signed_out(state == DriveState::NeedsSignIn);
            if live.set_listed(state, words) {
                set.tell_drives();
            }
            if reconciles(pass) {
                live.drive.listing_changed();
            }
            let (fetch, set, live) = (Arc::clone(&fetch), set.clone(), Arc::clone(&live));
            tokio::spawn(async move {
                if let Ok(quota) = fetch.quota().await {
                    live.drive.set_quota(quota.used.0, quota.total.map(|t| t.0));
                    set.tell_drives();
                }
            });
        };
        let task = tokio::spawn(drive::keep_listing_told(
            journal,
            listing,
            self.config.drive_every,
            live.paused(),
            files.listing_wanted(),
            told,
        ));
        Ok(Running::Drive {
            task,
            limit,
            id: live.id.clone(),
            path,
        })
    }

    /// The app folder mirror of `account` over `replica`.
    fn start_folder<R: Replica + 'static>(
        &self,
        account: &AccountDir,
        replica: R,
    ) -> Result<Running<T>, String> {
        let wiring = &self.wiring;
        let journal =
            Journal::open(&wiring.paths.journal(account, SLUG)).map_err(|e| e.to_string())?;
        let folder = wiring.paths.storage_dir(account);
        // The person drops files here; making it now lets them see where.
        let _ = std::fs::create_dir_all(&folder);
        let dataset = FolderDataset::new(folder);
        let name = DatasetName {
            account: account.clone(),
            dataset: dataset.id(),
        };
        let engine = Engine::new(
            replica,
            dataset,
            journal,
            Wall::scaled(wiring.settings.time_scale),
        );
        let handle = wiring
            .hub
            .register(name.clone(), self.config.files_owners.clone());
        let driver = Driver::new(
            engine,
            handle,
            wiring.settings,
            wiring.network.clone(),
            Arc::new(Notify::new()),
            seed_of(&format!("{account}/{SLUG}")),
        );
        Ok(Running::Folder {
            name,
            task: tokio::spawn(driver.run()),
        })
    }

    /// Google Photos of `account`: the upload folder as a dataset, and the picker.
    fn start_google_photos(
        &self,
        account: &AccountDir,
        candidate: &Candidate,
    ) -> Result<Running<T>, String> {
        let wiring = &self.wiring;
        let (upload, picker) =
            google_photos_endpoints(candidate).ok_or("no Google Photos endpoint")?;
        let base = |url: &EndpointUrl| WebUrl::try_from(url).map_err(|_| "not a web endpoint");
        let upload = match upload {
            Some(endpoint) => Some(self.start_google_upload(account, candidate, endpoint)?),
            None => None,
        };
        let picker = match picker {
            Some(endpoint) => Some(Arc::new(PhotosPicker::new(
                picker_http(
                    Arc::clone(&wiring.accounts),
                    candidate.grant.clone(),
                    endpoint.url.clone(),
                )
                .map_err(|e| e.to_string())?,
                &base(&endpoint.url)?,
                wiring.paths.photos_picked_dir(account),
            ))),
            None => None,
        };
        Ok(Running::GooglePhotos { upload, picker })
    }

    /// The upload folder of a Google account as a dataset: new files are sent once.
    fn start_google_upload(
        &self,
        account: &AccountDir,
        candidate: &Candidate,
        upload: &porter_core::ServiceEndpoint,
    ) -> Result<(DatasetName, JoinHandle<()>), String> {
        let wiring = &self.wiring;
        let base = |url: &EndpointUrl| WebUrl::try_from(url).map_err(|_| "not a web endpoint");
        let api = PhotosApi::new(
            library_http(
                Arc::clone(&wiring.accounts),
                candidate.grant.clone(),
                upload.url.clone(),
            ),
            &base(&upload.url)?,
            wiring.clock.clone(),
        );
        let replica = UploadReplica::open(
            api,
            wiring.paths.photos_ledger(account),
            &self.config.google_album,
        )
        .map_err(|e| e.to_string())?;
        let folder = wiring.paths.photos_upload_dir(account);
        // The person drops files here; making it now lets them see where.
        let _ = std::fs::create_dir_all(&folder);
        let dataset = FolderDataset::named(folder, UPLOAD_SLUG).ok_or("not a dataset name")?;
        let journal = Journal::open(&wiring.paths.journal(account, UPLOAD_SLUG))
            .map_err(|e| e.to_string())?;
        let name = DatasetName {
            account: account.clone(),
            dataset: dataset.id(),
        };
        let engine = Engine::new(
            replica,
            dataset,
            journal,
            Wall::scaled(wiring.settings.time_scale),
        );
        let handle = wiring
            .hub
            .register(name.clone(), self.config.photos_owners.clone());
        let driver = Driver::new(
            engine,
            handle,
            wiring.settings,
            wiring.network.clone(),
            Arc::new(Notify::new()),
            seed_of(&format!("{account}/{UPLOAD_SLUG}")),
        );
        Ok((name, tokio::spawn(driver.run())))
    }

    fn start_photos(
        &self,
        account: &AccountDir,
        candidate: &Candidate,
        url: &porter_core::EndpointUrl,
    ) -> Result<Running<T>, String> {
        let wiring = &self.wiring;
        let replica = |folder: &str| {
            graph_replica(
                Arc::clone(&wiring.accounts),
                candidate.grant.clone(),
                url.clone(),
                folder,
                wiring.clock.clone(),
            )
            .map_err(|e| e.to_string())
        };
        let library = PhotoLibrary::open(
            wiring.paths.photos_dir(account),
            new_device(account, &wiring.nonce),
            Arc::clone(&wiring.millis),
        )
        .map_err(|e| e.to_string())?;
        let photos = PhotosWiring {
            hub: wiring.hub.clone(),
            paths: wiring.paths.clone(),
            settings: wiring.settings,
            network: wiring.network.clone(),
            owners: self.config.photos_owners.clone(),
        };
        let run = crate::datasets::photos::start(
            &photos,
            account,
            library,
            replica("Photos/Originals")?,
            replica("Photos/Metadata")?,
        )
        .map_err(|e| e.to_string())?;
        let names = [
            crate::datasets::photos::PhotoOriginals::SLUG,
            crate::datasets::photos::PhotoMetadata::SLUG,
        ]
        .into_iter()
        .filter_map(DatasetId::parse)
        .map(|dataset| DatasetName {
            account: account.clone(),
            dataset,
        })
        .collect();
        Ok(Running::Photos { names, run })
    }

    /// Stops a mirror: its datasets leave the hub, waiting for a cycle in flight, then its
    /// engines end. Its files and journals stay.
    async fn retire(&self, running: Running<T>) {
        for name in running.names() {
            self.wiring.hub.stop(&name).await;
        }
        match running {
            Running::Folder { task, .. } => task.abort(),
            Running::Drive {
                task, limit, id, ..
            } => {
                task.abort();
                limit.abort();
                if let Some(live) = self.drives.remove(&id)
                    && let Err(why) = live.take_away().await
                {
                    eprintln!("syncd: a drive's folder was not taken away: {why}");
                }
            }
            Running::GooglePhotos {
                upload: Some((_, task)),
                ..
            } => task.abort(),
            _ => {}
        }
    }

    /// Runs `tick` every `rescan`, for as long as the task lives; after a look that could not
    /// do all it meant to, again soon and then less often (a passing error is never a stop
    /// until the next rescan).
    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                failures = if self.tick().await {
                    0
                } else {
                    failures.saturating_add(1)
                };
                tokio::time::sleep(next_look(self.config.rescan, failures)).await;
            }
        })
    }
}

/// Whether the files on demand are reconciled with the listing after `pass`: only once it
/// ended caught up. What a pass cut short (offline, refused, failed) lacks is not gone from the
/// drive, so nothing cached or kept is dropped against it.
fn reconciles(pass: &Result<drive::Pass, crate::journal::JournalError>) -> bool {
    matches!(pass, Ok(pass) if pass.outcome == Listed::Done)
}

/// A drive replica that starts from a tree a journal kept.
trait WithFolders: Sized {
    fn with_folders(self, folders: porter_sync::Folders) -> Self;
}

impl<H: porter_http::Http> WithFolders for storage_graph::GraphReplica<H> {
    fn with_folders(self, folders: porter_sync::Folders) -> Self {
        storage_graph::GraphReplica::with_folders(self, folders)
    }
}

impl<H: porter_http::Http> WithFolders for storage_gdrive::GdriveReplica<H> {
    fn with_folders(self, folders: porter_sync::Folders) -> Self {
        storage_gdrive::GdriveReplica::with_folders(self, folders)
    }
}

/// Whether the dataset `kind` of `account` may be started or shown again now: not a drive held
/// while its account is removed.
fn may_run(drives: &DriveSet, kind: StorageKind, account: &AccountDir) -> bool {
    kind != StorageKind::Drive || !drives.is_held(account.as_str())
}

/// Each drive's folder name in `~/Cloud`, by account: its provider's ("OneDrive", "Google
/// Drive"), with the account's label when one provider has two.
fn drive_folders(candidates: &[Candidate]) -> BTreeMap<porter_core::AccountId, String> {
    let provider = |c: &Candidate| match app_folder_store(c) {
        Some(AppFolderStore::Graph(_)) => "OneDrive",
        _ => "Google Drive",
    };
    folder_names(
        candidates
            .iter()
            .map(|c| (c.account.clone(), provider(c), c.label.0.as_str())),
    )
}

/// The folder name of each `(account, provider, label)`: the provider's name, with the label
/// after it when a provider has two, a `/` shown as porter-drive shows it, and two that would
/// still be one name told apart as porter-drive tells a folder's items apart ("OneDrive (Work)",
/// "OneDrive (Work) 2"), the account first by id keeping it, so a folder is never shared and
/// keeps its name across restarts.
fn folder_names<'a>(
    drives: impl IntoIterator<Item = (porter_core::AccountId, &'a str, &'a str)>,
) -> BTreeMap<porter_core::AccountId, String> {
    let mut drives: Vec<_> = drives.into_iter().collect();
    drives.sort_by(|a, b| a.0.cmp(&b.0));
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, provider, _) in &drives {
        *count.entry(provider).or_default() += 1;
    }
    let bases: Vec<String> = drives
        .iter()
        .map(
            |(_, provider, label)| match count.get(provider).copied().unwrap_or(0) {
                0 | 1 => (*provider).to_owned(),
                _ => porter_drive::naming::shown_base(&format!("{provider} ({label})")),
            },
        )
        .collect();
    drives
        .into_iter()
        .zip(porter_drive::naming::assign(&bases))
        .map(|((account, _, _), folder)| (account, folder))
        .collect()
}

/// Two replica types, so one `match` can build either.
enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<T: GrantsLink, G> Drop for StorageSupervisor<T, G> {
    fn drop(&mut self) {
        for running in self.running.values() {
            match running {
                Running::Folder { task, .. } => task.abort(),
                Running::Drive { task, limit, .. } => {
                    task.abort();
                    limit.abort();
                }
                Running::GooglePhotos {
                    upload: Some((_, task)),
                    ..
                } => task.abort(),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_sync::RetryAfter;

    #[test]
    fn only_a_pass_that_ended_caught_up_reconciles_the_cache() {
        let ended = |outcome| -> Result<drive::Pass, crate::journal::JournalError> {
            Ok(drive::Pass {
                outcome,
                ..drive::Pass::default()
            })
        };
        assert!(reconciles(&ended(Listed::Done)));
        for cut in [
            Listed::Retry(RetryAfter(30)),
            Listed::NeedsConsent,
            Listed::Unauthorized,
            Listed::Gone,
        ] {
            assert!(!reconciles(&ended(cut)), "{cut:?}");
        }
        assert!(!reconciles(&Err(crate::journal::JournalError::Corrupt(
            "a test".into()
        ))));
    }

    #[tokio::test]
    async fn a_drive_held_while_its_account_is_removed_is_not_started() {
        let _root_guard = crate::testing::scratch("held-drive");
        let root = _root_guard.path().to_owned();
        let paths = crate::paths::Paths {
            journals: root.join("state/porter/sync"),
            mirrors: root.join("data/porter/vdir"),
            callers_system: root.join("none"),
            callers_user: root.join("none"),
            cloud: root.join("Cloud"),
            drive_cache: root.join("cache/porter/drives"),
            settings: root.join("none"),
        };
        let account = AccountDir::parse("one").expect("account");
        let drives = DriveSet::default();
        assert!(may_run(&drives, StorageKind::Drive, &account));
        drives.retire(&paths, &account).await.expect("held");
        assert!(!may_run(&drives, StorageKind::Drive, &account), "held");
        assert!(may_run(&drives, StorageKind::AppFolder, &account));
        drives.release("one");
        assert!(
            may_run(&drives, StorageKind::Drive, &account),
            "the account stays"
        );
        drives.retire(&paths, &account).await.expect("held");
        drives.forget("one");
        assert!(may_run(&drives, StorageKind::Drive, &account), "gone");
    }

    #[test]
    fn two_drives_never_share_a_folder() {
        let id = |n: &str| porter_core::AccountId::parse(n).expect("id");
        let names = folder_names([
            (id("acct-b"), "OneDrive", "Work"),
            (id("acct-a"), "OneDrive", "Work"),
            (id("acct-c"), "OneDrive", "a/b"),
            (id("acct-d"), "Google Drive", "Home"),
        ]);
        assert_eq!(names[&id("acct-a")], "OneDrive (Work)");
        assert_eq!(names[&id("acct-b")], "OneDrive (Work) 2");
        assert_eq!(names[&id("acct-c")], "OneDrive (a\u{FF0F}b)");
        assert_eq!(names[&id("acct-d")], "Google Drive");
    }
}
