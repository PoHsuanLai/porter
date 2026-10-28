//! accountd as one thing a program can run: [`Config`] says where everything is, [`Daemon::build`]
//! loads the caller tables, the provider files and the registry and serves `org.quire.Accounts1`,
//! and [`Daemon::run`] keeps it serving until told to stop. The binary is this and nothing more:
//! it reads its arguments, asks [`Config::from_env`] for the environment's answers and calls these.
//!
//! No other function in the library reads the environment. A program that embeds accountd builds
//! its [`Config`] with [`Config::new`] and the `with_*` methods, naming every path and the bus
//! itself.
//!
//! accountd holds identities, Spaces and Tailscale, and the AI keys, the agents' logins and the
//! local runtimes: all of its provider files are served, and its keyring namespace is
//! [`NAMESPACE`]. The keys inferd held before are copied in when accountd starts
//! ([`crate::import`]). Sync's sign-ins belong to syncd.

use crate::import::{Import, ImportError, OldStore, Outcome};
use crate::paths::{HONOURS_PROC_ROOT, Paths};
use crate::{FileAudit, NewsStore, Options, serve_with};
use porter_callers::choose_proc_root;
use porter_callers::load_callers;
use porter_callers::{CallerFileError, ProcCallers};
use porter_core::ProviderId;
use porter_core::store::RegistryStore;
use porter_daemon::xdg::PathError;
use porter_dbus::BusTarget;
use porter_families::DynProvider;
use porter_families::set::{FamilyIo, ProviderNames, load_specs, local_runtimes, served};
use porter_identities::SheetsDesk;
use porter_ids::clock::SystemClock;
use porter_provider::{Provider, ProviderSpec};
use porter_secrets::Namespace;
use porter_secrets::keysel::{AnyKeys, Chosen, KeysRefusal, Selection, select};
use porter_service::AccountService;
use porter_service_core::registry::Registry;
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

/// The keyring namespace inferd filed its keys under, which the import reads and then empties.
const OLD_NAMESPACE: &str = "porter.inferd";

/// The namespace of inferd's old keys.
fn old_namespace() -> Namespace {
    Namespace::parse(OLD_NAMESPACE).expect("a literal namespace")
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
            porter_callers::SystemCallers::choose(HONOURS_PROC_ROOT, |name| std::env::var_os(name));
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

/// The provider files accountd serves and the families that serve them.
pub(crate) struct Families {
    /// The family of each provider served.
    pub(crate) families: Vec<Box<dyn DynProvider>>,
    /// The local runtimes (Ollama, llama.cpp, LM Studio): no family, reported by the probe.
    pub(crate) local: Vec<ProviderSpec>,
    /// What a person reads for each provider, and which group of the page it is under.
    pub(crate) names: ProviderNames,
}

impl Families {
    /// The ids of the providers a family serves (what `accountd add` checks before it asks).
    pub(crate) fn served_ids(&self) -> Vec<ProviderId> {
        self.families
            .iter()
            .map(|family| Provider::spec(family).id.clone())
            .collect()
    }
}

/// Loads the provider files of `paths` and builds the family of each: the internet providers
/// (identities and Tailscale), the AI services and the assistants. The local runtimes have no
/// family and are reported by the probe.
pub(crate) fn families_of(paths: &Paths) -> Families {
    let loaded = load_specs(&paths.provider_layers());
    for (file, why) in &loaded.skipped {
        eprintln!("accountd: skipped provider file {}: {why}", file.display());
    }
    let names = ProviderNames::from_specs(&loaded.specs);
    let io = FamilyIo::system(
        porter_provider::ProviderSet::layered(loaded.specs.clone(), Vec::new()),
        paths.client_files(),
        Arc::new(porter_ids::clock::SystemClock),
    )
    .with_tailscale(tailscale(paths));
    let (families, unserved) = served(loaded.specs, &io);
    let local = local_runtimes(&unserved);
    for spec in unserved.iter().filter(|spec| !local.contains(spec)) {
        eprintln!("accountd: no family serves provider `{}` yet", spec.id);
    }
    Families {
        families,
        local,
        names,
    }
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
    #[error("cannot serve {bus}: {0}", bus = porter_wire::names::ACCOUNTS_BUS)]
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
    /// An inferd that still keeps its own keys was running at the start, so they were not copied:
    /// the daemon ends when it is gone, to be started again and copy them.
    waiting: bool,
}

/// Why [`Daemon::run`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunEnd {
    /// The shutdown completed.
    Stopped,
    /// The inferd that held the keys itself is gone: start again and copy them. The binary exits
    /// with [`EXIT_RESTART_FOR_KEYS`], which the unit restarts on.
    KeysReady,
}

/// The exit status of a daemon that ends so that it is started again (BSD's `EX_TEMPFAIL`): the
/// unit's `Restart=on-failure` starts it.
pub const EXIT_RESTART_FOR_KEYS: u8 = 75;

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
        let chosen = Chosen::open(keys.clone(), namespace(), "accountd")?;
        eprintln!("{}", chosen.said);
        let secrets = chosen.keys;
        let old_secrets = Chosen::open(keys, old_namespace(), "accountd")?.keys;
        let table = load_callers(&paths.callers_system, &paths.callers_user)?;

        let Families {
            families,
            local,
            names: provider_names,
            ..
        } = families_of(&paths);
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

        let store = FileStore::new(paths.registry_dir.clone());
        let connection = bus.connect().await.map_err(StartError::Bus)?;
        // inferd's old keys come in before the registry is read, so the service starts with them.
        let waiting = bring_in_old_keys(&paths, &old_secrets, &secrets, &store, &connection).await;
        let registry = match store.load().await {
            Ok(stored) => Registry::from_persisted(stored),
            Err(why) => return Err(StartError::RegistryRefused(store.refusal(&why))),
        };

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
        let saved = Arc::new(tokio::sync::Notify::new());
        let service = Arc::new(
            AccountService::new(families, registry, secrets.clone(), sheets, SystemClock)
                .with_local_runtimes(local)
                .with_store(NewsStore::new(store, Arc::clone(&saved)))
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
            launchers: Some(crate::launcher::Launchers::new(
                connection.clone(),
                crate::launcher::LoginTiming::new()
                    .with_audit(Arc::new(FileAudit::new(paths.audit.clone()))),
            )),
            saved: Some(saved),
        };
        serve_with(&connection, service, callers, options)
            .await
            .map_err(StartError::Serve)?;
        Ok(Self {
            connection,
            waiting,
        })
    }

    /// The daemon's connection to the bus.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Serves until `shutdown` completes, then closes the connection (the name is released). The
    /// binary passes a future that never completes: SIGTERM ends the process. A daemon that did
    /// not copy the keys of an inferd that was still running ends, closing its connection too,
    /// when that inferd goes ([`RunEnd::KeysReady`]).
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> RunEnd {
        let ended = tokio::select! {
            () = shutdown => RunEnd::Stopped,
            () = old_inferd_gone(&self.connection), if self.waiting => RunEnd::KeysReady,
        };
        // A connection already closed by the bus has nothing left to release.
        let _ = self.connection.close().await;
        ended
    }
}

/// The bus name of an inferd.
const INFERD_NAME: &str = porter_dbus::inference::INFERENCE_BUS;

/// Whether an inferd that still keeps its own keys is on the bus: the name has an owner, and the
/// owner serves the `org.quire.Inference1.Keys` interface the inferd that moved the keys here no
/// longer does. The name is asked about before anything is called, since a call to a name
/// nobody owns would start inferd.
pub(crate) async fn old_inferd_serves_keys(connection: &Connection) -> bool {
    let Ok(bus) = zbus::fdo::DBusProxy::new(connection).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(INFERD_NAME) else {
        return false;
    };
    if !bus.name_has_owner(name).await.unwrap_or(false) {
        return false;
    }
    let Ok(builder) = zbus::fdo::IntrospectableProxy::builder(connection)
        .destination(INFERD_NAME)
        .and_then(|builder| builder.path(porter_dbus::inference::INFERENCE_PATH))
    else {
        return false;
    };
    let Ok(proxy) = builder.build().await else {
        return false;
    };
    proxy
        .introspect()
        .await
        .is_ok_and(|xml| xml.contains("org.quire.Inference1.Keys"))
}

/// Resolves when no inferd that keeps its own keys is on the bus any more.
async fn old_inferd_gone(connection: &Connection) {
    use zbus::export::futures_core::Stream;
    let Ok(proxy) = zbus::fdo::DBusProxy::new(connection).await else {
        return;
    };
    let Ok(mut changes) = proxy.receive_name_owner_changed().await else {
        return;
    };
    loop {
        if !old_inferd_serves_keys(connection).await {
            return;
        }
        let next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)).await;
        if next.is_none() {
            return;
        }
    }
}

/// Copies inferd's old keys into accountd's store, once. Returns whether it was put off because
/// an inferd that keeps its own keys is running (it may still change them, so nothing is
/// copied). A copy that failed is said and tried again at the next start; the daemon serves
/// either way.
async fn bring_in_old_keys(
    paths: &Paths,
    old_secrets: &AnyKeys,
    secrets: &AnyKeys,
    store: &FileStore,
    connection: &Connection,
) -> bool {
    if old_inferd_serves_keys(connection).await {
        eprintln!(
            "accountd: inferd still keeps its own AI keys; they are copied when it is replaced"
        );
        return true;
    }
    let old = OldStore {
        registry_dir: paths.inferd_keys.clone(),
        audit: paths.inferd_keys_audit.clone(),
    };
    let import = Import {
        old: &old,
        old_secrets,
        secrets,
        store,
        audit: &paths.audit,
        marker: &paths.ai_import,
    };
    match import.run().await {
        Ok(Outcome::Imported(report)) => eprintln!(
            "accountd: copied inferd's AI keys: {} accounts, {} permits, {} switches, {} keys, \
             {} kept as they were, {} audit lines",
            report.accounts,
            report.grants,
            report.toggles,
            report.secrets,
            report.conflicts,
            report.audit_lines
        ),
        Ok(Outcome::NothingToImport | Outcome::AlreadyImported) => {}
        Err(why @ ImportError::Conflict(_)) => {
            eprintln!("accountd: inferd's AI keys were not copied, and nothing was changed: {why}");
        }
        Err(why) => eprintln!("accountd: inferd's AI keys were not copied yet: {why}"),
    }
    false
}
