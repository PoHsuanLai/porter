//! accountd as one thing a program can run: [`Config`] says where everything is, [`Daemon::build`]
//! loads the caller tables, the provider files and the registry and serves `org.quire.Accounts1`,
//! and [`Daemon::run`] keeps it serving until told to stop. The binary is this and nothing more:
//! it reads its arguments, asks [`Config::from_env`] for the environment's answers and calls these.
//!
//! No other function in the library reads the environment. A program that embeds accountd builds
//! its [`Config`] with [`Config::new`] and the `with_*` methods, naming every path and the bus
//! itself.
//!
//! [`add_account`] is the logic of `accountd add` (the command's arguments and its printing stay
//! in the binary).

use crate::add::{AddArgs, AddError, AddReport, StdTerminal, TerminalSheets};
use crate::clock::SystemClock;
use crate::keysel::{AnyKeys, Chosen, KEYS_VAR, KeysRefusal, Selection, TestKeys, select};
use crate::paths::{PROC_GATE, Paths};
use crate::{
    AppNames, BusSheets, FileAudit, FileStore, Options, ProviderNames, RelayRoots, SecretsDesk,
    load_callers, serve_with,
};
use porter_core::xdg::PathError;
use porter_daemon::ProcRoot;
use porter_dbus::{BusTarget, CallerFileError, ProcCallers};
use porter_service::{AccountService, Registry, RegistryStore};
use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use zbus::Connection;

/// The variable that names a directory to read callers from in place of `/proc` (a test build
/// only).
pub const PROC_ROOT_VAR: &str = "ACCOUNTD_PROC_ROOT";

/// Everything accountd needs to start, as typed values.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Config {
    /// Where accountd reads and writes.
    pub paths: Paths,
    /// Which secret store holds the credentials.
    pub keys: Selection,
    /// A directory to read callers from in place of `/proc`; `Some` only in a test build.
    pub proc_root: Option<PathBuf>,
    /// The person's runtime directory (an absolute path): where a process credential kept in a
    /// file is written. None refuses that way as unavailable.
    pub runtime_dir: Option<PathBuf>,
    /// The bus to serve on.
    pub bus: BusTarget,
}

impl Config {
    /// A configuration over `paths` on `bus`: the Secret Service, callers read from `/proc`, no
    /// runtime directory.
    pub fn new(paths: Paths, bus: BusTarget) -> Self {
        Self {
            paths,
            keys: Selection::SecretService,
            proc_root: None,
            runtime_dir: None,
            bus,
        }
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

    /// With this runtime directory.
    #[must_use]
    pub fn with_runtime_dir(mut self, dir: PathBuf) -> Self {
        self.runtime_dir = Some(dir);
        self
    }

    /// The configuration the process's environment gives, with `provider_dirs` (the command
    /// line's `--providers`) laid over the system's and the person's provider directories. The
    /// one place accountd reads the environment: `HOME` and the XDG directories,
    /// `ACCOUNTD_TAILSCALE_SOCKET`, `ACCOUNTD_KEYS`, `ACCOUNTD_PROC_ROOT` and `XDG_RUNTIME_DIR`.
    ///
    /// # Errors
    /// There is no home to keep state in, or the secret store the environment asks for is refused.
    pub fn from_env(provider_dirs: &[PathBuf]) -> Result<Self, StartError> {
        let paths = Paths::resolve(|name| std::env::var(name).ok(), provider_dirs)?;
        let keys_var = std::env::var(KEYS_VAR).ok();
        let keys = select(keys_var.as_deref(), TestKeys::THIS_BUILD)?;
        let proc = ProcRoot::choose(PROC_GATE, PROC_ROOT_VAR, |name| std::env::var_os(name))
            .into_fixture();
        // The XDG rule: a relative path is invalid and ignored.
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        Ok(Self {
            paths,
            keys,
            proc_root: proc,
            runtime_dir,
            bus: BusTarget::Session,
        })
    }
}

/// Why accountd did not start, or why `accountd add` could not begin.
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
    /// The registry file was refused. The text says so in plain words: both files, nothing
    /// changed. The daemon stops with [`accountd::EXIT_REGISTRY_REFUSED`](crate::EXIT_REGISTRY_REFUSED),
    /// the status `accountd.service` does not restart on.
    #[error("{0}")]
    RegistryRefused(String),
    /// The bus cannot be reached.
    #[error("no session bus: {0}")]
    Bus(#[source] zbus::Error),
    /// The bus objects or the name could not be served.
    #[error("cannot serve {bus}: {0}", bus = porter_dbus::ACCOUNTS_BUS)]
    Serve(#[source] zbus::Error),
}

impl StartError {
    /// The exit status for this failure: a refused registry has one of its own.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::RegistryRefused(_) => ExitCode::from(crate::EXIT_REGISTRY_REFUSED),
            _ => ExitCode::FAILURE,
        }
    }
}

/// accountd, serving `org.quire.Accounts1`.
#[derive(Debug)]
pub struct Daemon {
    connection: Connection,
}

impl Daemon {
    /// Loads everything and serves: the caller tables, the provider files, the registry (a
    /// registry it cannot read stops the start, and nothing is changed), then the bus objects and
    /// the name. When this returns the name is owned and calls are answered.
    ///
    /// # Errors
    /// See [`StartError`].
    pub async fn build(config: Config) -> Result<Self, StartError> {
        let Config {
            paths,
            keys,
            proc_root,
            runtime_dir,
            bus,
        } = config;
        // Which secret store, before anything reads or files a credential: refused here, not later.
        let chosen = Chosen::open(keys)?;
        eprintln!("accountd: {}", chosen.said);
        let secrets = chosen.keys;
        let table = load_callers(&paths.callers_system, &paths.callers_user)?;

        let loaded = crate::providers::load_specs(&paths.provider_layers());
        for (file, why) in &loaded.skipped {
            eprintln!("accountd: skipped provider file {}: {why}", file.display());
        }
        // The families read the clients files themselves (porter-oauth's registry); this names a
        // row of the person's own that tried to send a sign-in elsewhere (sec-4), whose endpoints
        // the registry sets aside.
        let own_clients = std::fs::read_to_string(&paths.clients_user)
            .ok()
            .and_then(|text| porter_provider::parse_clients(&text).ok())
            .unwrap_or_default();
        for row in own_clients.clients.iter().filter(|c| c.leaves_the_issuer()) {
            eprintln!(
                "accountd: {}: the {:?} client's own sign-in addresses are not the issuer's; \
                 the issuer's own are used",
                paths.clients_user.display(),
                row.issuer
            );
        }
        let app_names =
            AppNames::new(table.clone(), paths.applications.clone()).with_agents(&loaded.specs);
        let provider_names = ProviderNames::from_specs(&loaded.specs);
        let io = crate::providers::FamilyIo::system(
            porter_provider::ProviderSet::layered(loaded.specs.clone(), Vec::new()),
            paths.client_files(),
        )
        .with_tailscale(porter_tailscale::LocalApi::new(&paths.tailscale_socket));
        let (families, unserved) = crate::providers::served(loaded.specs, &io);
        let local = crate::providers::local_runtimes(&unserved);
        for spec in unserved.iter().filter(|s| !local.contains(s)) {
            eprintln!("accountd: no family serves provider `{}` yet", spec.id);
        }

        let store = FileStore::new(paths.registry_dir.clone());
        let registry = match store.load().await {
            Ok(stored) => Registry::from_persisted(stored),
            Err(why) => return Err(StartError::RegistryRefused(store.refusal(&why))),
        };

        let connection = bus.connect().await.map_err(StartError::Bus)?;
        let callers = Arc::new(match proc_root {
            Some(root) => {
                eprintln!(
                    "accountd: reading callers from {} (test-proc-root build)",
                    root.display()
                );
                ProcCallers::with_proc_root(connection.clone(), table, root)
            }
            None => ProcCallers::new(connection.clone(), table),
        });
        let sheets =
            BusSheets::new(connection.clone(), Arc::clone(&callers)).with_names(app_names.clone());
        let service = Arc::new(
            AccountService::new(families, registry, secrets.clone(), sheets, SystemClock)
                .with_local_runtimes(local)
                .with_store(store)
                .with_audit(FileAudit::new(paths.audit.clone())),
        );
        let desk = SecretsDesk::new(secrets, FileAudit::new(paths.audit.clone()), SystemClock);
        let options = Options {
            clients: Some(paths.clients_user.clone()),
            relay_roots: RelayRoots::Platform,
            keys: Some(Arc::new(desk)),
            app_names,
            provider_names,
            login: crate::LoginTiming {
                audit: Some(Arc::new(FileAudit::new(paths.audit.clone()))),
                ..crate::LoginTiming::default()
            },
            runtime_dir,
            // `spaces.json` beside `registry.json`.
            spaces: crate::SpacesStore::File(paths.registry_dir.clone()),
            // Tailscale's own socket, `/var/run/tailscale/tailscaled.sock` unless `paths` names
            // another.
            tailnet: Some(crate::TailnetWatch::new(porter_tailscale::LocalApi::new(
                &paths.tailscale_socket,
            ))),
        };
        serve_with(&connection, service, callers, options)
            .await
            .map_err(StartError::Serve)?;
        Ok(Self { connection })
    }

    /// The daemon's connection to the bus.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Serves until `shutdown` completes, then closes the connection (the name is released). The
    /// binary passes a future that never completes: SIGTERM ends the process.
    pub async fn run(self, shutdown: impl Future<Output = ()>) {
        shutdown.await;
        // A connection already closed by the bus has nothing left to release.
        let _ = self.connection.close().await;
    }
}

/// Why `accountd add` did not finish.
#[derive(Debug, thiserror::Error)]
pub enum AddCommandError {
    /// It could not begin: the paths, the secret store or the registry.
    #[error(transparent)]
    Start(#[from] StartError),
    /// The command itself: its arguments, the daemon running, the sign-in.
    #[error(transparent)]
    Add(#[from] AddError),
}

impl AddCommandError {
    /// The exit status for this failure.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Start(why) => why.exit_code(),
            Self::Add(_) => ExitCode::FAILURE,
        }
    }
}

/// `accountd add`: the provider files, the registry and the secret store the daemon uses, a
/// terminal for a sheet, and the bus name as the lock. `provider`, `allow` and `classes` are the
/// command's arguments as typed.
///
/// # Errors
/// See [`AddCommandError`].
pub async fn add_account(
    config: &Config,
    provider: &str,
    allow: &[String],
    classes: &[String],
) -> Result<AddReport, AddCommandError> {
    let secrets: AnyKeys = {
        // Which secret store, before anything reads or files a credential: refused here.
        let chosen = Chosen::open(config.keys.clone()).map_err(StartError::from)?;
        eprintln!("accountd: {}", chosen.said);
        chosen.keys
    };
    let paths = &config.paths;
    let args = AddArgs::parse(provider, allow, classes)?;
    let loaded = crate::providers::load_specs(&paths.provider_layers());
    for (file, why) in &loaded.skipped {
        eprintln!("accountd: skipped provider file {}: {why}", file.display());
    }
    let io = crate::providers::FamilyIo::system(
        porter_provider::ProviderSet::layered(loaded.specs.clone(), Vec::new()),
        paths.client_files(),
    )
    .with_tailscale(porter_tailscale::LocalApi::new(&paths.tailscale_socket));
    let (families, _unserved) = crate::providers::served(loaded.specs, &io);
    let served: Vec<_> = families
        .iter()
        .map(|family| porter_provider::Provider::spec(family).id.clone())
        .collect();

    // The lock. With no session bus no daemon can be reached either, so go on and say so.
    let session = config.bus.connect().await.ok();
    match &session {
        Some(connection) => crate::add::take_the_name(connection).await?,
        None => eprintln!("accountd: no session bus; cannot tell whether the daemon is running"),
    }

    let store = FileStore::new(paths.registry_dir.clone());
    let registry = match store.load().await {
        Ok(stored) => Registry::from_persisted(stored),
        Err(why) => return Err(StartError::RegistryRefused(store.refusal(&why)).into()),
    };
    let sheets = TerminalSheets::new(StdTerminal);
    let service = AccountService::new(families, registry, secrets, sheets.clone(), SystemClock)
        .with_store(store)
        .with_audit(FileAudit::new(paths.audit.clone()));
    Ok(crate::add::run(&service, &sheets, &served, &args).await?)
}
