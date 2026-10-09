//! syncd: the sync daemon (`org.quire.Sync1`, design/31 §4.1, §6).
//!
//! The binary reads its arguments, asks `Config::from_env` for the environment's answers, then
//! builds and runs the daemon, all in the library (`syncd::daemon`): it loads the caller tables
//! (the files accountd reads), serves `org.quire.Sync1` over the hub of running datasets, and
//! listens for accountd's `AccountRemoved` to wipe an account's journals and mirrors. The PIM
//! supervisor (W6e) keeps a calendar or address book mirror running for every Calendar or
//! Contacts grant syncd holds; with no grant `Datasets` answers an empty list and every name the
//! refusal `NoFittingAccount`. The storage supervisor keeps an app folder mirror running for
//! every Storage grant (class Files) on a Microsoft account and, with `SYNCD_PHOTOS=on`, the
//! Photos datasets for every one of class Photos. A test build (`test-proc-root`) reads
//! `SYNCD_RESCAN_S` as how often both supervisors read grants again; the default is ten minutes.
//!
//! What stays here is the network: NetworkManager is not read yet, so the network is taken as
//! unmetered and up.

use clap::Parser;
use std::process::ExitCode;
use syncd::scheduler::Network;
use syncd::{Config, Daemon};

/// porter's sync service (`org.quire.Sync1`).
#[derive(Debug, Parser)]
#[command(name = "syncd", version)]
struct Args {}

/// The daemon's one log path is standard error, prefixed with its name.
fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("syncd: {why}");
    ExitCode::FAILURE
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Args {} = Args::parse();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(why) => return fail(why),
    };
    // NetworkManager is not read yet: the network is taken as unmetered and up. The sender is
    // kept for as long as the daemon runs, so the receivers never see the value go.
    let (_network_keeps, network) = tokio::sync::watch::channel(Network::Unmetered);
    let daemon = match Daemon::build(config, network).await {
        Ok(daemon) => daemon,
        Err(why) => return fail(why),
    };
    // Serves until the session ends or the unit stops it (SIGTERM ends the process).
    daemon.run(std::future::pending()).await;
    ExitCode::SUCCESS
}
