//! accountd as one thing a program can run: [`Config`] says where everything is, [`Daemon::build`]
//! loads the caller tables, the provider files and the registry and serves `org.quire.Accounts1`,
//! and [`Daemon::run`] keeps it serving until told to stop. The binary is this and nothing more:
//! it reads its arguments, asks [`Config::from_env`] for the environment's answers and calls these.
//!
//! No other function in the library reads the environment. A program that embeds accountd builds
//! its [`Config`] with [`Config::new`] and the `with_*` methods, naming every path and the bus
//! itself.
//!
//! accountd holds identities, Spaces and Tailscale: its provider files are the internet ones, and
//! its keyring namespace is [`NAMESPACE`]. AI keys, agents and sync belong to inferd and syncd.

use crate::paths::{HONOURS_PROC_ROOT, Paths};
use crate::{FileAudit, Options, SheetsDesk, load_callers, serve_with};
use porter_daemon::choose_proc_root;
use porter_daemon::xdg::PathError;
use porter_dbus::BusTarget;
use porter_dbus::callers::{CallerFileError, ProcCallers};
use porter_families::set::{FamilyIo, ProviderNames, load_specs, served};
use porter_ids::clock::SystemClock;
use porter_secrets::Namespace;
use porter_secrets::keysel::{Chosen, KeysRefusal, Selection, select};
use porter_service::{AccountService, Registry};
use porter_sheet::ProviderGroup;
use porter_sheets::{AppNames, BusSheets};
use porter_store::FileStore;
use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use zbus::Connection;

/// The variable that names a directory to read callers from in place of `/proc` (a test build
/// only).
pub const PROC_ROOT_VAR: &str = "ACCOUNTD_PROC_ROOT";

/// The variable that names the key file in a `test-keys` build (`file:<absolute path>`).
pub const KEYS_VAR: &str = "ACCOUNTD_KEYS";

/// accountd's keyring namespace: the `service` its items are filed under.
pub const NAMESPACE: &str = "porter.accountd";

/// accountd's [`Namespace`].
fn namespace() -> Namespace {
    Namespace::parse(NAMESPACE).expect("a literal namespace")
}

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
    /// The bus to serve on.
    pub bus: BusTarget,
}

impl Config {
    /// A configuration over `paths` on `bus`: the Secret Service, callers read from `/proc`.
    pub fn new(paths: Paths, bus: BusTarget) -> Self {
        Self {
            paths,
            keys: Selection::SecretService,
            proc_root: None,
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

    /// The configuration the process's environment gives, with `provider_dirs` (the command
    /// line's `--providers`) laid over the system's and the person's provider directories. The
    /// one place accountd reads the environment: `HOME` and the XDG directories,
    /// `ACCOUNTD_TAILSCALE_SOCKET`, `ACCOUNTD_KEYS` and `ACCOUNTD_PROC_ROOT`.
    ///
    /// # Errors
    /// There is no home to keep state in, or the secret store the environment asks for is refused.
    pub fn from_env(provider_dirs: &[PathBuf]) -> Result<Self, StartError> {
        let mut paths = Paths::resolve(|name| std::env::var(name).ok(), provider_dirs)?;
        // A test build may read another system caller table (a jail cannot write /etc).
        let system =
            porter_daemon::SystemCallers::choose(HONOURS_PROC_ROOT, |name| std::env::var_os(name));
        if let Some(line) = system.notice("accountd") {
            eprintln!("{line}");
        }
        paths.callers_system = system.path().to_owned();
        let keys_var = std::env::var(KEYS_VAR).ok();
        let keys = select(keys_var.as_deref())?;
        let proc = choose_proc_root(HONOURS_PROC_ROOT, PROC_ROOT_VAR, |name| {
            std::env::var_os(name)
        })
        .into_honoured();
        Ok(Self {
            paths,
            keys,
            proc_root: proc,
            bus: BusTarget::Session,
        })
    }
}

/// Tailscale as `paths` names it: its socket, and its program looked for on `PATH` too.
fn tailscale(paths: &Paths) -> porter_tailscale::LocalApi {
    porter_tailscale::LocalApi::new(&paths.tailscale_socket).with_path(&paths.program_path)
}

/// Why accountd did not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
    /// The registry file was refused. The words name both files and say nothing
    /// was changed. The daemon stops with [`accountd::EXIT_REGISTRY_REFUSED`](crate::EXIT_REGISTRY_REFUSED),
    /// the status `accountd.service` does not restart on.
    #[error("{}", porter_daemon::registry_words(.0.file(), .0.backup(), .0.cause(), "accounts service"))]
    RegistryRefused(porter_store::RegistryRefusal),
    /// The bus cannot be reached.
    #[error("no session bus: {0}")]
    Bus(#[source] zbus::Error),
    /// The bus objects or the name could not be served.
    #[error("cannot serve {bus}: {0}", bus = porter_dbus::accounts::ACCOUNTS_BUS)]
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
            bus,
        } = config;
        // Which secret store, before anything reads or files a credential: refused here, not later.
        let chosen = Chosen::open(keys, namespace(), "accountd")?;
        eprintln!("{}", chosen.said);
        let secrets = chosen.keys;
        let table = load_callers(&paths.callers_system, &paths.callers_user)?;

        let loaded = load_specs(&paths.provider_layers());
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
        let app_names = AppNames::new(table.clone(), paths.applications.clone());
        let provider_names = ProviderNames::from_specs(&loaded.specs);
        let io = FamilyIo::system(
            porter_provider::ProviderSet::layered(loaded.specs.clone(), Vec::new()),
            paths.client_files(),
            Arc::new(porter_ids::clock::SystemClock),
        )
        .with_tailscale(tailscale(&paths));
        // accountd serves the internet providers (identities and Tailscale); the AI and agent
        // ones are inferd's.
        let ours: Vec<_> = loaded
            .specs
            .into_iter()
            .filter(|spec| spec.group() == ProviderGroup::Internet)
            .collect();
        let (families, unserved) = served(ours, &io);
        for spec in &unserved {
            eprintln!("accountd: no family serves provider `{}` yet", spec.id);
        }

        let store = FileStore::new(paths.registry_dir.clone());
        // Only the identities and Tailscale stay; the old file is kept once beside itself.
        let registry = match crate::presplit::load_identities(&store).await {
            Ok(stored) => Registry::from_persisted(stored),
            Err(why) => return Err(StartError::RegistryRefused(store.refusal(&why))),
        };

        let connection = bus.connect().await.map_err(StartError::Bus)?;
        let callers = Arc::new(match proc_root {
            #[cfg(feature = "test-proc-root")]
            Some(root) => {
                eprintln!(
                    "accountd: reading callers from {} (test-proc-root build)",
                    root.display()
                );
                ProcCallers::with_proc_root(connection.clone(), table, root)
            }
            _ => ProcCallers::new(connection.clone(), table),
        });
        let sheets =
            BusSheets::new(connection.clone(), Arc::clone(&callers)).with_names(app_names.clone());
        let service = Arc::new(
            AccountService::new(families, registry, secrets.clone(), sheets, SystemClock)
                .with_store(store)
                .with_audit(FileAudit::new(paths.audit.clone())),
        );
        // The picker of `Identities1.Offer` is the sheet host, and the password of
        // `TakePassword` is read from the same store.
        let identity_desk = SheetsDesk::new(
            BusSheets::new(connection.clone(), Arc::clone(&callers)).with_names(app_names.clone()),
            secrets,
            FileAudit::new(paths.audit.clone()),
            SystemClock,
        );
        let options = Options {
            clients: Some(paths.clients_user.clone()),
            app_names,
            provider_names,
            clock: None,
            // `spaces.json` beside `registry.json`.
            spaces: Some(paths.registry_dir.clone()),
            // Tailscale's own socket, `/var/run/tailscale/tailscaled.sock` unless `paths` names
            // another.
            tailnet: Some(crate::TailnetWatch::new(tailscale(&paths))),
            // `identity-uses.json` beside `registry.json`.
            uses: Some(paths.registry_dir.clone()),
            identities: Some(Arc::new(identity_desk)),
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
