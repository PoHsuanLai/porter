//! inferd: the AI broker (design/31 §4.1, §5.5): serves `org.quire.Inference1` on the session
//! bus. It reads `inferd.toml` (engine programs, the `ai.*` settings rows, the caller table), the stoker
//! catalog, and runs engines as child processes of its own (the systemd transient-unit host is
//! not built). Nothing starts an engine until a session asks for its model.

use clap::Parser;
use engine_supervisor::EngineId;
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
use inferd::service::{Inference, serve_on};
use inferd::settings::{ConfigFile, InferdSettings, Reload, resolve, serve_settings};
use inferd::supervise::{Ports, Supervised};
use inferd::watch::Watch;
use model_catalog::EngineKind;
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

/// The configuration: a missing file is the default (no engines, no callers), an unreadable one
/// is an error.
fn read_config(path: &std::path::Path) -> Result<InferdConfig, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => InferdConfig::from_toml(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("inferd: no {}; no engines, no callers", path.display());
            Ok(InferdConfig::default())
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

async fn run(args: Args) -> Result<(), String> {
    let dirs = Dirs::from_env().map_err(|e| e.to_string())?;
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
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dirs.sockets)
        .map_err(|e| format!("{}: {e}", dirs.sockets.display()))?;
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
    let connection = zbus::connection::Builder::session()
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;
    // The hosted models of the catalogue, reached through accounts accountd holds the keys of.
    let cloud = Cloud::new(
        Arc::new(PeerAccountd::new(connection.clone())),
        &catalog.entries,
        Ledger::open(dirs.spend.clone()),
        Arc::new(SystemClock),
    );
    let engines = Engines::new(
        models,
        supervised,
        settings.settings.policy.clone(),
        settings.settings.tiers.clone(),
    )
    .with_settings(settings.settings)
    .with_cloud(cloud);
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
    let daemon = Inference::new(
        engines,
        Arc::clone(&peers),
        JsonLines::new(dirs.audit),
        SystemClock,
    )
    .limited(structured.limits)
    .reloading(reload.clone())
    .probing(probing);
    serve_on(&connection, daemon)
        .await
        .map_err(|e| e.to_string())?;
    serve_settings(&connection, InferdSettings::new(peers, reload))
        .await
        .map_err(|e| e.to_string())?;
    std::future::pending::<()>().await;
    Ok(())
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
