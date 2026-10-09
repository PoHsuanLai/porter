//! inferd: the AI broker (design/31 §4.1, §5.5): serves `org.quire.Inference1` on the session
//! bus. It reads `inferd.toml` (engine programs, the `ai.*` settings rows, the caller table), the stoker
//! catalog, and runs engines as child processes of its own (the systemd transient-unit host is
//! not built). Nothing starts an engine until a session asks for its model. `SIGTERM` and `SIGINT`
//! stop it gracefully: the bus name is released, every engine's process group is ended, and it
//! exits 0 (`inferd::shutdown`).

use clap::Parser;
use engine_supervisor::EngineId;
use inferd::agent::Agents;
use inferd::attached::{AddedFile, AttachedBook};
use inferd::audit::JsonLines;
use inferd::catalog::read_catalog;
use inferd::clock::SystemClock;
use inferd::cloud::Cloud;
use inferd::cloud::accountd::PeerAccountd;
use inferd::cloud::spend::Ledger;
use inferd::config::{Dirs, InferdConfig};
use inferd::engines::Engines;
use inferd::hosts::{HealthProbe, NvidiaSmi, ProcessHost};
use inferd::local::build;
use inferd::peers::{ProcGate, ProcPeers, ProcRoot};
use inferd::replay::Replays;
use inferd::report::PeerReports;
use inferd::service::{Inference, serve_with_settings};
use inferd::settings::{ConfigFile, InferdSettings, Reload, resolve};
use inferd::shutdown::{self, Ended, Signals};
use inferd::supervise::{Ports, Supervised};
use inferd::watch::Watch;
use model_catalog::EngineKind;
use porter_dbus::INFERENCE_BUS;
use porter_http::{HyperHttp, Limits};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

/// How often the daemon looks at its file for a change by someone else.
const RELOAD_EVERY: Duration = Duration::from_secs(2);

/// How long a runtime may take to answer a probe: one that does not is not running.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The most a runtime's model list may hold, in bytes.
const PROBE_MAX_BODY: usize = 1024 * 1024;

/// porter's AI broker (`org.quire.Inference1`).
#[derive(Debug, Parser)]
#[command(name = "inferd", version)]
struct Args {
    /// The configuration file, instead of `$XDG_CONFIG_HOME/quire/inferd.toml`.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
}

/// Why the daemon did not start or did not stop cleanly. Each keeps what failed; its text is
/// the line `main` logs.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// The signal handlers could not be set up.
    #[error("signal handlers: {0}")]
    Signals(#[source] std::io::Error),
    /// There is nowhere to put the sockets and files.
    #[error(transparent)]
    Dirs(#[from] inferd::config::ConfigError),
    /// The configuration file could not be read.
    #[error("{}: {source}", path.display())]
    ConfigUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The configuration file is not acceptable.
    #[error("{}: {source}", path.display())]
    ConfigRefused {
        path: PathBuf,
        #[source]
        source: inferd::config::ConfigError,
    },
    /// The attached engines the file names are refused.
    #[error(transparent)]
    Attached(#[from] inferd::attached::AttachedError),
    /// The sockets' directory could not be made.
    #[error("{}: {source}", path.display())]
    SocketDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The session bus.
    #[error(transparent)]
    Bus(#[from] zbus::Error),
    /// Shutdown took longer than its bound.
    #[error("shutdown took longer than {} s; every engine killed", .0.as_secs())]
    ShutdownTimedOut(Duration),
    /// A second signal came during shutdown.
    #[error("second signal during shutdown; every engine killed")]
    Hurried,
}

/// The configuration: a missing file is the default (no engines, no callers), an unreadable one
/// is an error.
fn read_config(path: &std::path::Path) -> Result<InferdConfig, RunError> {
    match std::fs::read_to_string(path) {
        Ok(text) => InferdConfig::from_toml(&text).map_err(|source| RunError::ConfigRefused {
            path: path.to_owned(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("inferd: no {}; no engines, no callers", path.display());
            Ok(InferdConfig::default())
        }
        Err(source) => Err(RunError::ConfigUnreadable {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The engines the file attaches, as models; a key file that is refused is said now (and again, as
/// the reason, whenever a session asks for the engine). The token is never read into the line.
fn attached_models(
    named: &[inferd::attached::Attached],
    added: &AddedFile,
    entries: &[model_catalog::ModelEntry],
    dirs: &Dirs,
) -> Result<Vec<inferd::local::LocalModel>, RunError> {
    // The computers Settings added are attached engines too; the settings file wins a clash.
    let (extra, said) = inferd::attached::computers::merged(named, added);
    for line in said {
        eprintln!("inferd: {line}");
    }
    let all: Vec<_> = named.iter().chain(&extra).cloned().collect();
    let models = inferd::attached::models(&all, entries, &dirs.sockets)?;
    for model in &models {
        let key = model
            .attached
            .as_ref()
            .and_then(|target| target.key.as_ref());
        if let Some(Err(why)) = key.map(inferd::attached::KeyFile::read) {
            eprintln!("inferd: attached engine {}: {why}", model.spec.id.0);
        }
    }
    Ok(models)
}

async fn run(args: Args) -> Result<(), RunError> {
    // Listening starts before anything else: a signal during start-up is kept, not fatal.
    let mut signals = Signals::listen().map_err(RunError::Signals)?;
    let dirs = Dirs::from_env()?;
    let config_path = args.config.clone().unwrap_or_else(|| dirs.config.clone());
    let config = read_config(&config_path)?;
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
    // An engine the person already runs is used and never started: an id they attached is not
    // also one inferd starts, and none of the attached is given to the supervisor.
    let named = config.engines.attached()?;
    let added = match AddedFile::read(&dirs.computers) {
        Ok(file) => file,
        Err(why) => {
            eprintln!("inferd: {why}");
            AddedFile::default()
        }
    };
    let attached = attached_models(&named, &added, &catalog.entries, &dirs)?;
    models.retain(|model| {
        attached
            .iter()
            .all(|one| one.entry.id.0 != model.entry.id.0)
    });
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dirs.sockets)
        .map_err(|source| RunError::SocketDir {
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
    let connection = zbus::connection::Builder::session()?.build().await?;
    // The hosted models of the catalogue, reached through accounts accountd holds the keys of.
    let cloud = Cloud::new(
        Arc::new(PeerAccountd::new(connection.clone())),
        &catalog.entries,
        Ledger::open(dirs.spend.clone()),
        Arc::new(SystemClock),
    );
    let book =
        AttachedBook::new(attached).with_catalogue(catalog.entries.clone(), dirs.sockets.clone());
    book.set_labels(added.labels());
    let engines = Engines::new(
        models,
        supervised,
        settings.settings.policy.clone(),
        settings.settings.tiers.clone(),
    )
    .with_settings(settings.settings)
    .with_cloud(cloud)
    .with_attached(book);
    // The runtimes the person runs themselves: looked for now, on `Rescan` and on a timer, and
    // reported to accountd as accounts.
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
    let reload = Reload::new(ConfigFile::new(config_path), engines.clone());
    reload.clone().watch(RELOAD_EVERY);
    let root = ProcRoot::select(
        ProcGate::BUILT,
        std::env::var("INFERD_PROC_ROOT").ok().as_deref(),
    );
    if let Some(line) = root.notice() {
        eprintln!("{line}");
    }
    let peers = Arc::new(ProcPeers::with_root(
        connection.clone(),
        config.callers.clone(),
        &root,
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
    .probing(probing);
    // Every object, the settings module among them, is served before the name is claimed.
    serve_with_settings(&connection, daemon, InferdSettings::new(peers, reload)).await?;
    let release = async {
        // No new caller finds the daemon; a failure to release is no reason not to stop engines.
        if let Err(e) = connection.release_name(INFERENCE_BUS).await {
            eprintln!("inferd: releasing {INFERENCE_BUS}: {e}");
        }
    };
    match shutdown::on_signal(&mut signals, &closer, release, shutdown::BOUND).await {
        Ended::Done => Ok(()),
        Ended::TimedOut => Err(RunError::ShutdownTimedOut(shutdown::BOUND)),
        Ended::Hurried => Err(RunError::Hurried),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // The daemon's one log path is standard error, prefixed with its name.
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("inferd: {why}");
            ExitCode::from(1)
        }
    }
}
