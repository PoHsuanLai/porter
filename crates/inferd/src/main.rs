//! inferd: the AI broker (design/31 §4.1, §5.5): serves `org.quire.Inference1` on the session
//! bus. It reads `inferd.toml` (engine programs, the `ai.*` settings rows, the caller table), the stoker
//! catalog, and runs engines as child processes of its own (the systemd transient-unit host is
//! not built). Nothing starts an engine until a session asks for its model. `SIGTERM` and `SIGINT`
//! stop it gracefully: the bus name is released, every engine's process group is ended, and it
//! exits 0 (`inferd::shutdown`).
//!
//! The binary reads its arguments, starts listening for signals, asks `Config::from_env` for the
//! environment's answers, then builds and runs the daemon, all in the library
//! (`inferd::daemon`).

use clap::Parser;
use inferd::shutdown::Signals;
use inferd::{Config, Daemon};
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

/// The daemon's one log path is standard error, prefixed with its name.
fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("inferd: {why}");
    ExitCode::from(1)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    // Listening starts before anything else: a signal during start-up is kept, not fatal.
    let signals = match Signals::listen() {
        Ok(signals) => signals,
        Err(why) => return fail(format_args!("signal handlers: {why}")),
    };
    let config = match Config::from_env(args.config) {
        Ok(config) => config,
        Err(why) => return fail(why),
    };
    let daemon = match Daemon::build(config).await {
        Ok(daemon) => daemon,
        Err(why) => return fail(why),
    };
    match daemon.run(signals).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => fail(why),
    }
}
