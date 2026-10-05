//! inferd: the AI broker (design/31 §4.1, §5.5): serves `org.quire.Inference1` on the session
//! bus. It reads `inferd.toml` (engine programs, policy, tier map, the caller table), the stoker
//! catalog, and runs engines as child processes of its own (the systemd transient-unit host is
//! not built). Nothing starts an engine until a session asks for its model.

use clap::Parser;
use engine_supervisor::{EngineId, SupervisorConfig};
use inferd::audit::JsonLines;
use inferd::catalog::read_catalog;
use inferd::clock::SystemClock;
use inferd::config::{Dirs, InferdConfig};
use inferd::engines::Engines;
use inferd::hosts::{HealthProbe, NvidiaSmi, ProcessHost};
use inferd::local::build;
use inferd::peers::ProcPeers;
use inferd::replay::Replays;
use inferd::service::{Inference, serve_on};
use inferd::supervise::{Ports, Supervised};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::ExitCode;

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
    let config = read_config(&args.config.clone().unwrap_or_else(|| dirs.config.clone()))?;
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
    let sockets: Vec<(EngineId, PathBuf)> = models
        .iter()
        .map(|model| (model.spec.id.clone(), model.socket.0.clone()))
        .collect();
    let supervised = Supervised::start(
        specs,
        replays.supervisor_config(models.len()),
        Ports {
            host: replays.host(ProcessHost::new()),
            probe: HealthProbe::new(sockets),
            gpu: NvidiaSmi::new(PathBuf::from("nvidia-smi")),
        },
    );
    let structured = config.ai.resolve();
    for path in &structured.rejected {
        eprintln!("inferd: {path}: out of range; using its default");
    }
    let engines = Engines::new(models, supervised, config.policy(), config.tiers.clone());
    let connection = zbus::connection::Builder::session()
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;
    let peers = ProcPeers::new(connection.clone(), config.callers.clone());
    let daemon = Inference::new(engines, peers, JsonLines::new(dirs.audit), SystemClock)
        .limited(structured.limits);
    serve_on(&connection, daemon)
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
