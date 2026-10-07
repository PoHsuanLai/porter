//! The supervisor of Storage: every `rescan` it asks accountd which grants syncd holds (for each
//! [`StorageKind`]) and keeps one running mirror per granted account and kind.
//!
//! - `AppFolder` (`DataClass::Files`): the account's app folder as a two-way [`FolderDataset`]
//!   at `$XDG_DATA_HOME/porter/storage/<account>/`, one dataset `<account>/storage_app_folder`
//!   (owner `org.quire.Files`), over a [`graph_replica`] relay of the grant for a OneDrive
//!   account or a [`gdrive_replica`] relay for a Google one. Which one is read from the
//!   endpoint the account's candidate lists ([`app_folder_store`]), never from the provider.
//! - `Photos` (`DataClass::Photos`): the two Photos datasets over `Photos/Originals` and
//!   `Photos/Metadata` of the app folder of a Microsoft account, only when the [`PhotosSwitch`]
//!   is on.
//! - `GooglePhotos` (`DataClass::Photos`, a `Photos`-kind grant): Google Photos, only when the
//!   switch is on: the upload folder `$XDG_DATA_HOME/porter/photos/<account>/upload/` as the
//!   dataset `<account>/google_photos_upload` (owner `org.quire.Photos`), and the picker the
//!   Photos app imports through ([`StorageSupervisor::google_picker`]).
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
use crate::clock::SystemClock;
use crate::dataset::{Dataset, DatasetId};
use crate::datasets::photos::google::{
    GooglePicker, PhotosApi, PhotosPicker, SLUG as UPLOAD_SLUG, UploadReplica, library_http,
    picker_http,
};
use crate::datasets::photos::{
    DeviceId, PhotoLibrary, PhotosRun, PhotosSwitch, PhotosWiring, SystemMillis,
};
use crate::datasets::pim::Wiring;
use crate::driver::Driver;
use crate::engine::Engine;
use crate::gdrive::gdrive_replica;
use crate::graph::graph_replica;
use crate::journal::Journal;
use crate::paths::AccountDir;
use crate::service::{Access, DatasetName};
use porter_client::Transport;
use porter_core::{AppName, Candidate, EndpointUrl, WebUrl};
use porter_sync::Replica;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use storage_graph::Clock;
use tokio::sync::Notify;
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
    /// Whether the Photos datasets run (off by default: there is no Photos app yet).
    pub photos: PhotosSwitch,
    /// Who owns the app folder dataset.
    pub files_owners: Access,
    /// Who owns the Photos datasets.
    pub photos_owners: Access,
    /// The title of the album Google Photos uploads go into (an album the app creates).
    pub google_album: String,
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
            photos: PhotosSwitch::Off,
            files_owners: owned_by(FILES_APP),
            photos_owners: owned_by(PHOTOS_APP),
            google_album: crate::datasets::photos::google::ALBUM_TITLE.to_owned(),
        }
    }
}

enum Running<T: Transport> {
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
    /// Google Photos: the upload engine, and the picker when the account has one.
    GooglePhotos {
        name: DatasetName,
        task: JoinHandle<()>,
        picker: Option<Arc<GooglePicker<T>>>,
    },
}

impl<T: Transport> std::fmt::Debug for Running<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

impl<T: Transport> Running<T> {
    fn names(&self) -> Vec<DatasetName> {
        match self {
            Running::Folder { name, .. } | Running::GooglePhotos { name, .. } => {
                vec![name.clone()]
            }
            Running::Photos { names, .. } => names.clone(),
        }
    }
}

/// Keeps the Storage mirrors of every granted account.
#[derive(Debug)]
pub struct StorageSupervisor<T: Transport, G> {
    wiring: Wiring<T>,
    grants: G,
    config: StorageConfig,
    running: BTreeMap<(AccountDir, StorageKind), Running<T>>,
    silent: BTreeSet<StorageKind>,
}

/// A device name for this machine's Photos library, made once (the library keeps it).
fn new_device(account: &AccountDir) -> DeviceId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let digest = Sha256::digest(format!("{account}/{nanos}/{}", std::process::id()));
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    DeviceId::parse(&format!("d{hex}")).expect("a device name")
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
    T: Transport + 'static,
    G: StorageGrants + 'static,
{
    /// A supervisor over `wiring` (its `owners` are not used: `config` names the owners of each
    /// kind), asking `grants`.
    pub fn new(wiring: Wiring<T>, grants: G, config: StorageConfig) -> Self {
        Self {
            wiring,
            grants,
            config,
            running: BTreeMap::new(),
            silent: BTreeSet::new(),
        }
    }

    /// The Photos library of `account` while its Photos datasets run: what the Photos app will
    /// import into and read from (and a test's way to put a photo in).
    pub fn photos_library(&self, account: &AccountDir) -> Option<&PhotoLibrary> {
        match self.running.get(&(account.clone(), StorageKind::Photos))? {
            Running::Photos { run, .. } => Some(run.library()),
            Running::Folder { .. } | Running::GooglePhotos { .. } => None,
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
            Running::Folder { .. } | Running::Photos { .. } => None,
        }
    }

    /// The datasets running now.
    pub fn datasets(&self) -> Vec<DatasetName> {
        self.running.values().flat_map(Running::names).collect()
    }

    /// One look: grants, what to start and what to stop.
    pub async fn tick(&mut self) {
        for kind in StorageKind::ALL {
            if kind.is_photos() && self.config.photos == PhotosSwitch::Off {
                continue;
            }
            let Ok(candidates) = self.grants.granted(kind).await else {
                continue;
            };
            if candidates.is_empty() && self.silent.insert(kind) {
                eprintln!(
                    "syncd: no {kind:?} grant on an account it mirrors, so no {kind:?} is mirrored"
                );
            }
            let mut granted: Vec<AccountDir> = Vec::new();
            for candidate in &candidates {
                let Some(account) =
                    AccountDir::parse(&porter_core::object_segment(&candidate.account))
                else {
                    continue;
                };
                granted.push(account.clone());
                let key = (account.clone(), kind);
                if self.running.contains_key(&key) {
                    continue;
                }
                match self.start(kind, &account, candidate) {
                    Ok(running) => {
                        self.running.insert(key, running);
                    }
                    Err(why) => eprintln!("syncd: cannot mirror {kind:?} of {account}: {why}"),
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
    }

    fn start(
        &self,
        kind: StorageKind,
        account: &AccountDir,
        candidate: &Candidate,
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
                        Clock::system(),
                    )
                    .map(Either::Left),
                    AppFolderStore::Drive(e) => gdrive_replica(
                        Arc::clone(&wiring.accounts),
                        grant(),
                        e.url.clone(),
                        "",
                        storage_gdrive::Clock::system(),
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
        }
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
        let engine = Engine::new(replica, dataset, journal, SystemClock);
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
        let api = PhotosApi::new(
            library_http(
                Arc::clone(&wiring.accounts),
                candidate.grant.clone(),
                upload.url.clone(),
            ),
            &base(&upload.url)?,
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
        let engine = Engine::new(replica, dataset, journal, SystemClock);
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
        Ok(Running::GooglePhotos {
            name,
            task: tokio::spawn(driver.run()),
            picker,
        })
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
                Clock::system(),
            )
            .map_err(|e| e.to_string())
        };
        let library = PhotoLibrary::open(
            wiring.paths.photos_dir(account),
            new_device(account),
            Arc::new(SystemMillis),
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
            self.config.photos,
            account,
            library,
            replica("Photos/Originals")?,
            replica("Photos/Metadata")?,
        )
        .map_err(|e| e.to_string())?
        .ok_or("the Photos switch is off")?;
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
        if let Running::Folder { task, .. } | Running::GooglePhotos { task, .. } = running {
            task.abort();
        }
    }

    /// Runs `tick` every `rescan`, for as long as the task lives.
    pub fn spawn(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                self.tick().await;
                tokio::time::sleep(self.config.rescan).await;
            }
        })
    }
}

/// Two replica types, so one `match` can build either.
enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<T: Transport, G> Drop for StorageSupervisor<T, G> {
    fn drop(&mut self) {
        for running in self.running.values() {
            if let Running::Folder { task, .. } | Running::GooglePhotos { task, .. } = running {
                task.abort();
            }
        }
    }
}
