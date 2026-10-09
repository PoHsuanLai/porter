//! syncd as one thing a program can run: [`Config`] says where everything is, [`Daemon::build`]
//! loads the caller tables, starts the supervisors that keep the mirrors and serves
//! `org.quire.Sync1`, and [`Daemon::run`] keeps it going until told to stop. The binary reads
//! its arguments, asks [`Config::from_env`] for the environment's answers and calls these.
//!
//! No other function in the library reads the environment. The network comes from the caller as
//! a `watch::Receiver<Network>`: NetworkManager is not read yet, so the binary hands in a
//! receiver that says "unmetered and up" for good.

use crate::datasets::photos::PhotosSwitch;
use crate::datasets::pim::{ClientGrants, PimConfig, PimSupervisor, Wiring};
use crate::datasets::storage::{ClientStorageGrants, StorageConfig, StorageSupervisor};
use crate::paths::{BUILD, Paths, proc_root, rescan};
use crate::scheduler::{Network, Settings};
use crate::service::{Access, Hub};
use crate::{removal, service};
use porter_client::{Accounts, ClientError, DbusTransport};
use porter_core::xdg::PathError;
use porter_dbus::{BusTarget, CallerFileError, ProcCallers};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zbus::Connection;

/// The variable that names a directory to read callers from in place of `/proc` (a test build
/// only).
pub const PROC_ROOT_VAR: &str = "SYNCD_PROC_ROOT";

/// The variable that shortens how often grants are read again (a test build only).
pub const RESCAN_VAR: &str = "SYNCD_RESCAN_S";

/// The variable that turns the Photos datasets on (`on`).
pub const PHOTOS_VAR: &str = "SYNCD_PHOTOS";

/// Everything syncd needs to start, as typed values.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Config {
    /// Where syncd reads and writes.
    pub paths: Paths,
    /// A directory to read callers from in place of `/proc`; `Some` only in a test build.
    pub proc_root: Option<PathBuf>,
    /// The calendar and address book supervisor.
    pub pim: PimConfig,
    /// The storage supervisor, with the Photos switch.
    pub storage: StorageConfig,
    /// The bus to serve on.
    pub bus: BusTarget,
}

impl Config {
    /// A configuration over `paths` on `bus`: callers read from `/proc`, every supervisor at its
    /// shipped settings (grants read again every ten minutes, no Photos).
    pub fn new(paths: Paths, bus: BusTarget) -> Self {
        Self {
            paths,
            proc_root: None,
            pim: PimConfig::default(),
            storage: StorageConfig::default(),
            bus,
        }
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
    /// environment: `HOME` and the XDG directories, `SYNCD_PROC_ROOT` and `SYNCD_RESCAN_S` (a
    /// test build only) and `SYNCD_PHOTOS`.
    ///
    /// # Errors
    /// There is no home to keep state in.
    pub fn from_env() -> Result<Self, StartError> {
        let paths = Paths::resolve(|name| std::env::var(name).ok())?;
        let proc = proc_root(BUILD, std::env::var(PROC_ROOT_VAR).ok());
        // `SYNCD_RESCAN_S` shortens how often grants are read again, in a test build only.
        let rescan_var = std::env::var(RESCAN_VAR).ok();
        let pim = PimConfig::default();
        let pim = PimConfig {
            rescan: rescan(BUILD, rescan_var.clone(), pim.rescan),
        };
        // Storage over Graph: the app folder mirror for a Files grant, and Photos for a Photos
        // grant when `SYNCD_PHOTOS=on` (it is off otherwise: there is no Photos app yet).
        let storage = StorageConfig::default();
        let storage = StorageConfig {
            rescan: rescan(BUILD, rescan_var, storage.rescan),
            photos: PhotosSwitch::from_var(std::env::var(PHOTOS_VAR).ok().as_deref()),
            ..storage
        };
        Ok(Self {
            paths,
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
    /// A caller table cannot be read.
    #[error("{0}")]
    Callers(#[from] CallerFileError),
    /// The bus cannot be reached.
    #[error("no session bus: {0}")]
    Bus(#[source] zbus::Error),
    /// accountd's `AccountRemoved` cannot be listened for.
    #[error("cannot listen for AccountRemoved: {0}")]
    Removals(#[source] ClientError),
    /// `org.quire.Photos1.Picker` cannot be served.
    #[cfg(feature = "photos-picker")]
    #[error("cannot serve org.quire.Photos1.Picker: {0}")]
    Picker(#[source] zbus::Error),
    /// The bus objects or the name could not be served.
    #[error("cannot serve {bus}: {0}", bus = porter_dbus::SYNC_BUS)]
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
    /// Loads the caller tables, starts the supervisors, listens for `AccountRemoved` and serves
    /// `org.quire.Sync1` (and, with the `photos-picker` feature and `SYNCD_PHOTOS=on`,
    /// `org.quire.Photos1.Picker`). When this returns the name is owned and calls are answered.
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
            proc_root,
            pim,
            storage,
            bus,
        } = config;
        let table = porter_dbus::load_callers(&paths.callers_system, &paths.callers_user)?;
        let connection = bus.connect().await.map_err(StartError::Bus)?;
        let callers = Arc::new(match proc_root {
            Some(root) => {
                eprintln!(
                    "syncd: reading callers from {} (test-proc-root build)",
                    root.display()
                );
                ProcCallers::with_proc_root(connection.clone(), table, root)
            }
            None => ProcCallers::new(connection.clone(), table),
        });
        let hub = Hub::default();
        let accounts = Arc::new(Accounts::over(DbusTransport::over(connection.clone())));
        let wiring = |network| Wiring {
            accounts: Arc::clone(&accounts),
            hub: hub.clone(),
            paths: paths.clone(),
            settings: Settings::default(),
            network,
            owners: Access::default(),
        };
        let pim = PimSupervisor::new(
            wiring(network.clone()),
            ClientGrants::new(Arc::clone(&accounts)),
            pim,
        )
        .spawn();
        #[cfg(feature = "photos-picker")]
        let photos_on = storage.photos == PhotosSwitch::On;
        #[cfg(feature = "photos-picker")]
        let photos_owners = storage.photos_owners.clone();
        let storage = StorageSupervisor::new(
            wiring(network),
            ClientStorageGrants::new(Arc::clone(&accounts)),
            storage,
        );
        #[cfg(feature = "photos-picker")]
        let pickers = storage.pickers();
        let storage = storage.spawn();
        let daemon = Self {
            connection: connection.clone(),
            pim,
            storage,
        };
        // On an error below `daemon` drops, which ends both supervisors.
        removal::watch(&connection, hub.clone(), paths)
            .await
            .map_err(StartError::Removals)?;
        // `org.quire.Photos1.Picker` is there only while Google Photos runs (`SYNCD_PHOTOS=on`),
        // and only in a build with the `photos-picker` feature.
        #[cfg(feature = "photos-picker")]
        if photos_on {
            service::serve_picker(&connection, pickers, photos_owners, Arc::clone(&callers))
                .await
                .map_err(StartError::Picker)?;
        }
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
