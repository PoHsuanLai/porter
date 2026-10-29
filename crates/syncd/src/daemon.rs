//! syncd as one thing a program can run: [`Config`] says where everything is, [`Daemon::build`]
//! loads the caller tables, starts the supervisors that keep the mirrors and serves
//! `org.quire.Sync1`, and [`Daemon::run`] keeps it going until told to stop. The binary reads
//! its arguments, asks [`Config::from_env`] for the environment's answers and calls these.
//!
//! No other function in the library reads the environment. The network comes from the caller as
//! a `watch::Receiver<Network>`: NetworkManager is not read yet, so the binary hands in a
//! receiver that says "unmetered and up" for good.

use crate::datasets::photos::Millis;
use crate::datasets::pim::{ClientGrants, PimConfig, PimSupervisor, Wiring};
use crate::datasets::storage::DriveMount;
use crate::datasets::storage::{ClientStorageGrants, StorageConfig, StorageSupervisor};
use crate::keep::{FileAudit, Intent, Keep, SyncSheets, sync_app};
use crate::paths::{BUILD, HONOURS_PROC_ROOT, KeepPaths, Paths, rescan};
use crate::scheduler::{Network, Settings};
use crate::service;
use crate::service::{Access, Hub};
use crate::settings::{SyncSettings, read_cache_limit, serve_settings};
use porter_callers::choose_proc_root;
use porter_callers::{CallerFileError, ProcCallers};
use porter_client::Porter;
use porter_client_dbus::Bus;
use porter_client_inproc::{Hosted, PorterRelays};
use porter_core::store::RegistryStore;
use porter_daemon::xdg::PathError;
use porter_dbus::BusTarget;
use porter_drive_fuse::Mounter;
use porter_families::DynProvider;
use porter_families::set::{FamilyIo, ProviderNames, load_specs, served};
use porter_ids::clock::SystemClock;
use porter_provider::ProviderSet;
use porter_secrets::Namespace;
use porter_secrets::keysel::{AnyKeys, Chosen, KeysRefusal, Selection, select};
use porter_service::AccountService;
use porter_service_core::registry::Registry;
use porter_sheet::ProviderGroup;
use porter_sheets::BusSheets;
use porter_store::FileStore;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zbus::Connection;

/// The wall clock in milliseconds, for Photos' hybrid clocks: the daemon is where it is read.
#[derive(Debug, Clone, Copy)]
struct SystemMillis;

impl Millis for SystemMillis {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

/// A value that differs between two runs of the daemon: the time it started, in nanoseconds,
/// and its process id, in hex.
fn fresh_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:x}{:x}", std::process::id())
}

/// The variable that names a directory to read callers from in place of `/proc` (a test build
/// only).
pub const PROC_ROOT_VAR: &str = "SYNCD_PROC_ROOT";

/// The variable that shortens how often grants are read again (a test build only).
pub const RESCAN_VAR: &str = "SYNCD_RESCAN_S";

/// The variable that names a key file in place of the Secret Service (a `test-keys` build only).
pub const KEYS_VAR: &str = "SYNCD_KEYS";

/// The keyring namespace syncd's own credentials are filed under.
pub const KEYRING_NAMESPACE: &str = "porter.syncd";

/// The service syncd hosts for its own accounts, over its own store, keys and sheets.
pub type OwnService = AccountService<
    Box<dyn DynProvider>,
    AnyKeys,
    SyncSheets<BusSheets<ProcCallers>>,
    SystemClock,
    FileStore,
    FileAudit,
>;

/// What the supervisors reach accounts through: that service, in this process, with relays that
/// carry the credential to the servers (it crosses no process).
pub type OwnLink = Hosted<OwnService, PorterRelays>;

/// Everything syncd needs to start, as typed values.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Config {
    /// Where syncd reads and writes.
    pub paths: Paths,
    /// Where it keeps its own accounts.
    pub keep: KeepPaths,
    /// Which secret store holds its credentials.
    pub keys: Selection,
    /// A directory to read callers from in place of `/proc`; `Some` only in a test build.
    pub proc_root: Option<PathBuf>,
    /// The calendar and address book supervisor.
    pub pim: PimConfig,
    /// The storage supervisor.
    pub storage: StorageConfig,
    /// The bus to serve on.
    pub bus: BusTarget,
}

impl Config {
    /// A configuration over `paths` on `bus`: callers read from `/proc`, every supervisor at its
    /// shipped settings (grants read again every ten minutes).
    pub fn new(paths: Paths, bus: BusTarget) -> Self {
        // Its own accounts beside the journals unless `with_keep` says where.
        let keep = KeepPaths::under(&paths.journals.with_file_name("syncd-own"));
        Self {
            paths,
            keep,
            keys: Selection::SecretService,
            proc_root: None,
            pim: PimConfig::default(),
            storage: StorageConfig::default(),
            bus,
        }
    }

    /// With its own accounts kept at these paths.
    #[must_use]
    pub fn with_keep(mut self, keep: KeepPaths) -> Self {
        self.keep = keep;
        self
    }

    /// With this secret store.
    #[must_use]
    pub fn with_keys(mut self, keys: Selection) -> Self {
        self.keys = keys;
        self
    }

    /// With callers read from this directory in place of `/proc`.
    #[must_use]
    pub fn with_proc_root(mut self, root: PathBuf) -> Self {
        self.proc_root = Some(root);
        self
    }

    /// With these supervisor settings.
    #[must_use]
    pub fn with_supervisors(mut self, pim: PimConfig, storage: StorageConfig) -> Self {
        self.pim = pim;
        self.storage = storage;
        self
    }

    /// The configuration the process's environment gives. The one place syncd reads the
    /// environment: `HOME` and the XDG directories, `SYNCD_PROC_ROOT`, `SYNCD_RESCAN_S` and
    /// `SYNCD_KEYS` (a test build only).
    ///
    /// # Errors
    /// There is no home to keep state in, or the secret store the environment asks for is refused.
    pub fn from_env() -> Result<Self, StartError> {
        let mut paths = Paths::resolve(|name| std::env::var(name).ok())?;
        // A test build may read another system caller table (a jail cannot write /etc).
        let system =
            porter_callers::SystemCallers::choose(HONOURS_PROC_ROOT, |name| std::env::var_os(name));
        if let Some(line) = system.notice("syncd") {
            eprintln!("{line}");
        }
        paths.callers_system = system.path().to_owned();
        let keep = KeepPaths::resolve(|name| std::env::var(name).ok())?;
        let keys_var = std::env::var(KEYS_VAR).ok();
        let keys = select(keys_var.as_deref())?;
        let proc = choose_proc_root(HONOURS_PROC_ROOT, PROC_ROOT_VAR, |name| {
            std::env::var_os(name)
        })
        .into_honoured();
        // `SYNCD_RESCAN_S` shortens how often grants are read again, in a test build only.
        let rescan_var = std::env::var(RESCAN_VAR).ok();
        let pim = PimConfig::default();
        let pim = PimConfig {
            rescan: rescan(BUILD, rescan_var.clone(), pim.rescan),
        };
        // Storage: the app folder mirror for a Files grant, the whole drive for a grant of all of
        // it ("Show in Files"), and Photos for a Photos grant (the account's Photos row).
        let storage = StorageConfig::default();
        // Each drive shown in Files is a folder at `~/Cloud/<Drive>`, mounted by porter-mountd
        // (the unit's sandbox may not run the setuid `fusermount3`, nor make a mount anyone else
        // sees), or by syncd itself through `fusermount3` when no helper listens (syncd run by
        // hand); its cache keeps what `syncd.toml` says.
        let mounter = Mounter {
            mountd: std::env::var_os("XDG_RUNTIME_DIR")
                .map(|runtime| porter_drive_fuse::mountd::wire::socket_in(Path::new(&runtime))),
            ..Mounter::new("fusermount3")
        };
        let storage = StorageConfig {
            rescan: rescan(BUILD, rescan_var, storage.rescan),
            mount: DriveMount::Fuse(mounter),
            cache_limit: read_cache_limit(&paths.settings),
            ..storage
        };
        Ok(Self {
            paths,
            keep,
            keys,
            proc_root: proc,
            pim,
            storage,
            bus: BusTarget::Session,
        })
    }
}

/// Why syncd did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// There is no home to keep state in.
    #[error("{0}")]
    Paths(#[from] PathError),
    /// The secret store asked for is refused.
    #[error("{0}")]
    Keys(#[from] KeysRefusal),
    /// A caller table cannot be read.
    #[error("{0}")]
    Callers(#[from] CallerFileError),
    /// syncd's own registry file was refused. The words name both files and say
    /// nothing was changed.
    #[error("{}", porter_daemon::registry_words(.0.file(), .0.backup(), .0.cause(), "sync service"))]
    RegistryRefused(porter_store::RegistryRefusal),
    /// The bus cannot be reached.
    #[error("no session bus: {0}")]
    Bus(#[source] zbus::Error),
    /// The settings module cannot be served.
    #[error("cannot serve the settings module: {0}")]
    Settings(#[source] zbus::Error),
    /// `org.quire.Photos1.Picker` cannot be served.
    #[cfg(feature = "photos-picker")]
    #[error("cannot serve org.quire.Photos1.Picker: {0}")]
    Picker(#[source] zbus::Error),
    /// The bus objects or the name could not be served.
    #[error("cannot serve {bus}: {0}", bus = porter_dbus::sync::SYNC_BUS)]
    Serve(#[source] zbus::Error),
}

/// syncd, serving `org.quire.Sync1` with its supervisors running.
#[derive(Debug)]
pub struct Daemon {
    connection: Connection,
    pim: JoinHandle<()>,
    storage: JoinHandle<()>,
}

impl Daemon {
    /// Loads the caller tables, opens syncd's own accounts (its registry and secrets, hosted in
    /// process), starts the supervisors, serves the settings module and serves
    /// `org.quire.Sync1` (and, with the `photos-picker` feature, `org.quire.Photos1.Picker`). When this returns the name is owned and calls are answered.
    /// `network` is what the supervisors read the network from.
    ///
    /// # Errors
    /// See [`StartError`].
    pub async fn build(
        config: Config,
        network: watch::Receiver<Network>,
    ) -> Result<Self, StartError> {
        let Config {
            paths,
            keep,
            keys,
            proc_root,
            pim,
            storage,
            bus,
        } = config;
        // Which secret store, before anything reads or files a credential: refused here, not later.
        let namespace = Namespace::parse(KEYRING_NAMESPACE).expect("a literal namespace");
        let chosen = Chosen::open(keys, namespace, "syncd")?;
        eprintln!("{}", chosen.said);
        let table = porter_callers::load_callers(&paths.callers_system, &paths.callers_user)?;
        let connection = bus.connect().await.map_err(StartError::Bus)?;
        let callers = Arc::new(match proc_root {
            #[cfg(feature = "test-proc-root")]
            Some(root) => {
                eprintln!(
                    "syncd: reading callers from {} (test-proc-root build)",
                    root.display()
                );
                ProcCallers::with_proc_root(connection.clone(), table, root)
            }
            _ => ProcCallers::new(connection.clone(), table),
        });
        let hub = Hub::default();

        // syncd's own accounts: the identity families only (what the person signs in to on the
        // internet), over syncd's own registry, secrets and audit file.
        let loaded = load_specs(&keep.provider_layers());
        for (file, why) in &loaded.skipped {
            eprintln!("syncd: skipped provider file {}: {why}", file.display());
        }
        let specs: Vec<_> = loaded
            .specs
            .into_iter()
            .filter(|spec| spec.group() == ProviderGroup::Internet)
            .collect();
        let names = ProviderNames::from_specs(&specs);
        let io = FamilyIo::system(
            ProviderSet::layered(specs.clone(), Vec::new()),
            keep.client_files(),
            Arc::new(SystemClock),
        );
        let (families, _unserved) = served(specs, &io);
        let store = FileStore::new(keep.registry_dir.clone());
        let registry = match store.load().await {
            Ok(stored) => Registry::from_persisted(stored),
            Err(why) => return Err(StartError::RegistryRefused(store.refusal(&why))),
        };
        let intent = Intent::default();
        let sheets = SyncSheets::new(
            BusSheets::new(connection.clone(), Arc::clone(&callers)),
            intent.clone(),
        );
        let service: Arc<OwnService> = Arc::new(
            AccountService::new(families, registry, chosen.keys, sheets, SystemClock)
                .with_store(store)
                .with_audit(FileAudit::new(keep.audit.clone())),
        );
        let link: OwnLink =
            Hosted::new(Arc::clone(&service), sync_app()).with_relays(PorterRelays::new());
        let accounts = Arc::new(Porter::over(link).grants());
        // What tells this run from every other: its anchors, its new Photos devices.
        let nonce = fresh_nonce();
        let wiring = |network| Wiring {
            accounts: Arc::clone(&accounts),
            hub: hub.clone(),
            paths: paths.clone(),
            settings: Settings::default(),
            network,
            owners: Access::default(),
            clock: porter_sync::Clock::new(SystemClock),
            nonce: nonce.clone(),
            millis: Arc::new(SystemMillis),
        };
        let pim = PimSupervisor::new(
            wiring(network.clone()),
            ClientGrants::new(Arc::clone(&accounts)),
            pim,
        )
        .spawn();
        #[cfg(feature = "photos-picker")]
        let photos_owners = storage.photos_owners.clone();
        let storage_files_apps = storage.files_owners.clone();
        let storage = StorageSupervisor::new(
            wiring(network),
            ClientStorageGrants::new(Arc::clone(&accounts)),
            storage,
        );
        #[cfg(feature = "photos-picker")]
        let pickers = storage.pickers();
        let (drives, cache_knob) = (storage.drive_set(), storage.cache_knob());
        let files_apps = storage_files_apps.clone();
        let storage = storage.spawn();
        let daemon = Self {
            connection: connection.clone(),
            pim,
            storage,
        };
        // On an error below `daemon` drops, which ends both supervisors.
        // The Settings rows: the accounts syncd signed in, "Continue as" for a new one, the sync
        // switches and removal.
        let identities = Porter::over(Bus::over(connection.clone())).identities();
        let settings_file = paths.settings.clone();
        let own = Arc::new(
            Keep::new(Arc::clone(&service), identities, intent, paths, hub.clone())
                .with_drives(drives.clone()),
        );
        serve_settings(
            &connection,
            SyncSettings::new(own, Arc::clone(&callers), names)
                .with_cache_limit(cache_knob, settings_file),
        )
        .await
        .map_err(StartError::Settings)?;
        // `org.quire.Photos1.Picker`, only in a build with the `photos-picker` feature; each
        // account answers through it while its Google Photos run.
        #[cfg(feature = "photos-picker")]
        service::serve_picker(&connection, pickers, photos_owners, Arc::clone(&callers))
            .await
            .map_err(StartError::Picker)?;
        // `org.quire.Storage1`: the drives shown in Files.
        service::serve_storage(&connection, drives, Arc::clone(&callers), files_apps)
            .await
            .map_err(StartError::Serve)?;
        service::serve(&connection, hub, callers)
            .await
            .map_err(StartError::Serve)?;
        Ok(daemon)
    }

    /// The daemon's connection to the bus.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Serves until `shutdown` completes, then ends the supervisors and closes the connection
    /// (the name is released). The binary passes a future that never completes: SIGTERM ends
    /// the process.
    pub async fn run(mut self, shutdown: impl Future<Output = ()>) {
        shutdown.await;
        self.pim.abort();
        self.storage.abort();
        // An aborted task ends with a cancellation; there is nothing else to learn from it.
        let _ = (&mut self.pim).await;
        let _ = (&mut self.storage).await;
        // A connection already closed by the bus has nothing left to release.
        let _ = self.connection.clone().close().await;
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.pim.abort();
        self.storage.abort();
    }
}
