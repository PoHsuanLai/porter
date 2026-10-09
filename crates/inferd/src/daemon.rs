//! inferd as one thing a program can run: [`Config`] says where everything is, [`Daemon::build`]
//! reads the configuration and the catalog, starts the engine supervisor and the watchers and
//! serves `org.quire.Inference1`, and [`Daemon::run`] keeps it going until a stop is asked for
//! and then ends every engine. The binary reads its arguments, starts listening for signals, asks
//! [`Config::from_env`] for the environment's answers and calls these.
//!
//! No other function in the library reads the environment (`config::Dirs::from_env` is the
//! directories' half of [`Config::from_env`], and [`Config::new`] takes them from the caller).

use crate::agent::Agents;
use crate::attached::{AddedFile, Attached, AttachedBook, Computers};
use crate::audit::JsonLines;
use crate::catalog::read_catalog;
use crate::clock::SystemClock;
use crate::cloud::Cloud;
use crate::cloud::accountd::PeerAccountd;
use crate::cloud::spend::Ledger;
use crate::config::{ConfigError, Dirs, InferdConfig};
use crate::engines::Engines;
use crate::hosts::{HealthProbe, HostCloser, NvidiaSmi, ProcessHost};
use crate::local::{LocalModel, build};
use crate::peers::{ProcGate, ProcPeers, ProcRoot};
use crate::replay::Replays;
use crate::report::PeerReports;
use crate::service::{Inference, serve_with_settings};
use crate::settings::{ConfigFile, InferdSettings, Reload, resolve};
use crate::shutdown::{self, Ended, Shutdown};
use crate::supervise::{Ports, Supervised};
use crate::tailnet::{BusMachines, Parts, Tailnet};
use crate::watch::Watch;
use engine_supervisor::EngineId;
use model_catalog::EngineKind;
use porter_dbus::{BusTarget, INFERENCE_BUS};
use porter_http::{HyperHttp, Limits};
use porter_tailnet::{Config as TailnetConfig, Guests, Timing};
use porter_tailscale::LocalApi;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use zbus::Connection;

/// How often the daemon looks at its file for a change by someone else.
const RELOAD_EVERY: Duration = Duration::from_secs(2);

/// How long a runtime may take to answer one probe request. One that does not is not marked
/// offline for it alone: `watch::confirm` waits for several such looks in a row, so a loaded
/// computer's slow answer does not end a runtime's models.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most a runtime's model list may hold, in bytes.
const PROBE_MAX_BODY: usize = 1024 * 1024;

/// The variable that names Tailscale's socket (an absolute path; a relative one is ignored).
pub const TAILSCALE_SOCKET_VAR: &str = "INFERD_TAILSCALE_SOCKET";

/// The variable that names a directory to read callers from in place of `/proc` (a test build
/// only).
pub const PROC_ROOT_VAR: &str = "INFERD_PROC_ROOT";

/// Everything inferd needs to start, as typed values.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Config {
    /// Where the catalog, the sockets, the audit file and the settings are.
    pub dirs: Dirs,
    /// The configuration file (`inferd.toml`).
    pub config_file: PathBuf,
    /// Tailscale's socket; none means the path Tailscale itself uses.
    pub tailscale_socket: Option<PathBuf>,
    /// Where the caller lookup reads processes.
    pub proc_root: ProcRoot,
    /// The bus to serve on.
    pub bus: BusTarget,
}

impl Config {
    /// A configuration over `dirs` on `bus`: the configuration file at `dirs.config`, Tailscale
    /// at its usual socket, callers read from `/proc`.
    pub fn new(dirs: Dirs, bus: BusTarget) -> Self {
        Self {
            config_file: dirs.config.clone(),
            dirs,
            tailscale_socket: None,
            proc_root: ProcRoot::System,
            bus,
        }
    }

    /// With the configuration file at `path`.
    #[must_use]
    pub fn with_config_file(mut self, path: PathBuf) -> Self {
        self.config_file = path;
        self
    }

    /// With Tailscale's socket at `path`.
    #[must_use]
    pub fn with_tailscale_socket(mut self, path: PathBuf) -> Self {
        self.tailscale_socket = Some(path);
        self
    }

    /// With callers read from this tree.
    #[must_use]
    pub fn with_proc_root(mut self, root: ProcRoot) -> Self {
        self.proc_root = root;
        self
    }

    /// The configuration the process's environment gives; `config_file` is the command line's
    /// `--config`, else the file under `$XDG_CONFIG_HOME`. The one place inferd reads the
    /// environment: `HOME`, the XDG directories and `HF_HOME` (`Dirs::from_env`),
    /// `INFERD_TAILSCALE_SOCKET` and `INFERD_PROC_ROOT`.
    ///
    /// # Errors
    /// There is nowhere to put the sockets and files.
    pub fn from_env(config_file: Option<PathBuf>) -> Result<Self, StartError> {
        let dirs = Dirs::from_env()?;
        let tailscale_socket = std::env::var_os(TAILSCALE_SOCKET_VAR)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let proc_root = ProcRoot::select(
            ProcGate::BUILT,
            std::env::var(PROC_ROOT_VAR).ok().as_deref(),
        );
        let mut config = Self::new(dirs, BusTarget::Session);
        if let Some(path) = config_file {
            config = config.with_config_file(path);
        }
        config.tailscale_socket = tailscale_socket;
        config.proc_root = proc_root;
        Ok(config)
    }
}

/// Why the daemon did not start. Each keeps what failed; its text is the line the binary logs.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// There is nowhere to put the sockets and files.
    #[error(transparent)]
    Dirs(#[from] ConfigError),
    /// The configuration file could not be read.
    #[error("{}: {source}", path.display())]
    ConfigUnreadable {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// The configuration file is not acceptable.
    #[error("{}: {source}", path.display())]
    ConfigRefused {
        /// The file.
        path: PathBuf,
        /// Why.
        #[source]
        source: ConfigError,
    },
    /// The attached engines the file names are refused.
    #[error(transparent)]
    Attached(#[from] crate::attached::AttachedError),
    /// The sockets' directory could not be made.
    #[error("{}: {source}", path.display())]
    SocketDir {
        /// The directory.
        path: PathBuf,
        /// Why.
        #[source]
        source: std::io::Error,
    },
    /// The bus.
    #[error(transparent)]
    Bus(#[from] zbus::Error),
}

/// Why the daemon did not stop cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StopError {
    /// Shutdown took longer than its bound.
    #[error("shutdown took longer than {} s; every engine killed", .0.as_secs())]
    TimedOut(Duration),
    /// A second request came during shutdown.
    #[error("second signal during shutdown; every engine killed")]
    Hurried,
}

/// The configuration: a missing file is the default (no engines, no callers), an unreadable one
/// is an error.
fn read_config(path: &Path) -> Result<InferdConfig, StartError> {
    match std::fs::read_to_string(path) {
        Ok(text) => InferdConfig::from_toml(&text).map_err(|source| StartError::ConfigRefused {
            path: path.to_owned(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("inferd: no {}; no engines, no callers", path.display());
            Ok(InferdConfig::default())
        }
        Err(source) => Err(StartError::ConfigUnreadable {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The engines the file attaches, as models; a key file that is refused is said now (and again, as
/// the reason, whenever a session asks for the engine). The token is never read into the line.
fn attached_models(
    named: &[Attached],
    added: &AddedFile,
    entries: &[model_catalog::ModelEntry],
    dirs: &Dirs,
) -> Result<Vec<LocalModel>, StartError> {
    // The computers Settings added are attached engines too; the settings file wins a clash.
    let (extra, said) = crate::attached::computers::merged(named, added, &dirs.sockets);
    for line in said {
        eprintln!("inferd: {line}");
    }
    let all: Vec<_> = named.iter().chain(&extra).cloned().collect();
    let models = crate::attached::models(&all, entries, &dirs.sockets)?;
    for model in &models {
        let key = model
            .attached
            .as_ref()
            .and_then(|target| target.key.as_ref());
        if let Some(Err(why)) = key.map(crate::attached::KeyFile::read) {
            eprintln!("inferd: attached engine {}: {why}", model.spec.id.0);
        }
    }
    Ok(models)
}

/// inferd, serving `org.quire.Inference1` with its engine supervisor and watchers running.
#[derive(Debug)]
pub struct Daemon {
    connection: Connection,
    closer: HostCloser,
}

impl Daemon {
    /// Reads the configuration and the catalog, starts the supervisor, the watchers and the
    /// tailnet, and serves `org.quire.Inference1`. Nothing starts an engine until a session asks
    /// for its model. When this returns the name is owned and calls are answered.
    ///
    /// # Errors
    /// See [`StartError`].
    pub async fn build(config: Config) -> Result<Self, StartError> {
        let Config {
            dirs,
            config_file,
            tailscale_socket,
            proc_root,
            bus,
        } = config;
        let config = read_config(&config_file)?;
        let catalog = read_catalog(&dirs.catalog);
        for skipped in &catalog.skipped {
            eprintln!("inferd: catalog: {}: {}", skipped.what, skipped.why);
        }
        let mut models = build(&catalog.entries, &config.engines_in(&dirs), &dirs.sockets);
        let replays = Replays::read(&config.engines.named, &dirs.sockets);
        for problem in replays.skipped.iter().chain(&replays.unread()) {
            eprintln!("inferd: replay engine {problem}");
        }
        models.extend(replays.models.iter().cloned());
        // An engine the person already runs is used and never started: an id they attached is
        // not also one inferd starts, and none of the attached is given to the supervisor.
        let named = config.engines.attached()?;
        let added = match AddedFile::read(&dirs.computers) {
            Ok(file) => file,
            Err(why) => {
                eprintln!("inferd: {why}");
                AddedFile::default()
            }
        };
        let mut attached = attached_models(&named, &added, &catalog.entries, &dirs)?;
        let relayed = |one: &LocalModel| {
            one.attached
                .as_ref()
                .is_some_and(|target| target.is_relayed())
        };
        // A model another computer lends is not also one this computer runs: the one here stays.
        attached.retain(|one| {
            let clash = relayed(one)
                && models
                    .iter()
                    .any(|model| model.entry.id.0 == one.entry.id.0);
            if clash {
                eprintln!(
                    "inferd: {} is also run on this computer; the one here is used",
                    one.entry.id.0
                );
            }
            !clash
        });
        models.retain(|model| {
            attached
                .iter()
                .filter(|one| !relayed(one))
                .all(|one| one.entry.id.0 != model.entry.id.0)
        });
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dirs.sockets)
            .map_err(|source| StartError::SocketDir {
                path: dirs.sockets.clone(),
                source,
            })?;
        let specs = models.iter().map(|model| model.spec.clone()).collect();
        let speech_hosts: Vec<EngineId> = models
            .iter()
            .filter(|model| model.profile.kind == EngineKind::SpeechHost)
            .map(|model| model.spec.id.clone())
            .collect();
        let sockets: Vec<(EngineId, PathBuf)> = models
            .iter()
            .map(|model| (model.spec.id.clone(), model.socket.0.clone()))
            .collect();
        let processes = ProcessHost::new().with_sockets(sockets.clone());
        let diagnostics = processes.diagnostics();
        let closer = processes.closer();
        let supervised = Supervised::start_with(
            specs,
            replays.supervisor_config(models.len()),
            Ports {
                host: replays.host(processes),
                probe: HealthProbe::new(sockets).speech_hosts(speech_hosts),
                gpu: NvidiaSmi::new(PathBuf::from("nvidia-smi")),
            },
            diagnostics,
        );
        let structured = config.ai.resolve();
        for path in &structured.rejected {
            eprintln!("inferd: {path}: out of range; using its default");
        }
        let settings = resolve(&config);
        for path in &settings.rejected {
            eprintln!("inferd: {path}: not accepted; using its default");
        }
        let connection = bus.connect().await?;
        // The hosted models of the catalogue, reached through accounts accountd holds the keys of.
        let cloud = Cloud::new(
            Arc::new(PeerAccountd::new(connection.clone())),
            &catalog.entries,
            Ledger::open(dirs.spend.clone()),
            Arc::new(SystemClock),
        );
        let book = AttachedBook::new(attached)
            .with_catalogue(catalog.entries.clone(), dirs.sockets.clone());
        book.set_labels(added.labels());
        let computers = Computers::new(
            dirs.computers.clone(),
            dirs.computer_keys.clone(),
            book.clone(),
            added,
            &named,
        );
        let engines = Engines::new(
            models,
            supervised,
            settings.settings.policy.clone(),
            settings.settings.tiers.clone(),
        )
        .with_settings(settings.settings)
        .with_cloud(cloud)
        .with_attached(book);
        // The person's other computers on their Tailscale network: lent to while
        // `ai.tailnet.serve` is on, looked for when Settings asks, and reached through a relay
        // each. Nothing asks Tailscale until one of those happens.
        let (guests, said) = Guests::open(&dirs.guests);
        if let Some(line) = said {
            eprintln!("inferd: {line}");
        }
        let tailnet = Tailnet::start(
            Parts {
                api: tailscale_socket.map_or_else(LocalApi::system, LocalApi::new),
                machines: Arc::new(BusMachines::new(connection.clone())),
                guests: Arc::new(guests),
                sockets: dirs.sockets.clone(),
                config: TailnetConfig::product(),
                timing: Timing::usual(),
            },
            engines.clone(),
            Arc::new(JsonLines::new(dirs.audit.clone())),
            Arc::new(SystemClock),
        );
        for node in computers.tailnet_nodes().keys() {
            if let Err(why) = tailnet.ensure_relay(node) {
                eprintln!("inferd: the way to computer {node}: {why}");
            }
        }
        // The runtimes the person runs themselves: looked for now, on `Rescan` and on a timer,
        // and reported to accountd as accounts.
        let probing = Watch::new(
            HyperHttp::new().with_limits(Limits {
                timeout: PROBE_TIMEOUT,
                max_body: PROBE_MAX_BODY,
            }),
            config.probe.clone(),
            engines.clone(),
            Arc::new(PeerReports::new(connection.clone())),
            dirs.sockets.clone(),
        )
        .spawn();
        let reload = Reload::new(ConfigFile::new(config_file), engines.clone());
        reload.clone().watch(RELOAD_EVERY);
        if let Some(line) = proc_root.notice() {
            eprintln!("{line}");
        }
        let peers = Arc::new(ProcPeers::with_root(
            connection.clone(),
            config.callers.clone(),
            &proc_root,
        ));
        // The agent endpoints are always served; `ai.agents.endpoint` (off by default) decides
        // whether the launcher, who is in no table until a machine lists it, may open one.
        let agents = Agents::new(
            engines.clone(),
            Arc::new(JsonLines::new(dirs.audit.clone())),
            Arc::new(SystemClock),
        );
        let daemon = Inference::new(
            engines,
            Arc::clone(&peers),
            JsonLines::new(dirs.audit),
            SystemClock,
        )
        .agents(agents)
        .limited(structured.limits)
        .reloading(reload.clone())
        .probing(probing)
        .computers(computers)
        .tailnet(tailnet);
        // Every object, the settings module among them, is served before the name is claimed.
        serve_with_settings(&connection, daemon, InferdSettings::new(peers, reload)).await?;
        Ok(Self { connection, closer })
    }

    /// The daemon's connection to the bus.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Serves until `stop` asks for a stop; then releases the bus name (no new caller finds the
    /// daemon) and ends every engine's process group, all within `shutdown::BOUND`. A second
    /// request, or the bound running out, kills every group at once.
    ///
    /// # Errors
    /// See [`StopError`]: the stop was hurried, or took longer than its bound.
    pub async fn run<S: Shutdown>(self, mut stop: S) -> Result<(), StopError> {
        let Self { connection, closer } = self;
        let release = async {
            // A failure to release is no reason not to stop engines.
            if let Err(e) = connection.release_name(INFERENCE_BUS).await {
                eprintln!("inferd: releasing {INFERENCE_BUS}: {e}");
            }
        };
        let ended = shutdown::on_signal(&mut stop, &closer, release, shutdown::BOUND).await;
        // The name is released and the engines are gone; a connection the bus closed already has
        // nothing left to close.
        let _ = connection.close().await;
        match ended {
            Ended::Done => Ok(()),
            Ended::TimedOut => Err(StopError::TimedOut(shutdown::BOUND)),
            Ended::Hurried => Err(StopError::Hurried),
        }
    }
}
