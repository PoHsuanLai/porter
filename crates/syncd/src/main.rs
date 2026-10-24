//! syncd: the sync daemon (`org.quire.Sync1`, design/31 §4.1, §6).
//!
//! The binary reads its arguments, asks `Config::from_env` for the environment's answers, then
//! builds and runs the daemon, all in the library (`syncd::daemon`): it loads the caller tables
//! (the files accountd reads), opens its own accounts (it signs in for itself, "Continue as", and
//! keeps its own tokens under the keyring namespace `porter.syncd`), serves its Settings rows and
//! `org.quire.Sync1` over the hub of running datasets. The PIM
//! supervisor (W6e) keeps a calendar or address book mirror running for every Calendar or
//! Contacts grant syncd holds (its own grants, made by the sync switches); with no grant `Datasets` answers an empty list and every name the
//! refusal `NoFittingAccount`. The storage supervisor keeps an app folder mirror running for
//! every Storage grant (class Files) on a Microsoft account, and the Photos datasets for every one
//! of class Photos (the account's Photos row in Settings). A test build (`test-proc-root`) reads
//! `SYNCD_RESCAN_S` as how often both supervisors read grants again; the default is ten minutes.
//!
//! What stays here is the network: NetworkManager is not read yet, so the network is taken as
//! unmetered and up.
// The PIM supervisor's future nests the whole account service's generics; peephole's caller
// types (porter-dbus ProcCallers) take its layout past the default depth of 128.
#![recursion_limit = "256"]

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
    // Serves until the unit stops it (SIGTERM) or it is interrupted: the drives shown in Files
    // are unmounted on the way out.
    let shutdown = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => std::future::pending().await,
        }
    };
    daemon.run(shutdown).await;
    ExitCode::SUCCESS
}
