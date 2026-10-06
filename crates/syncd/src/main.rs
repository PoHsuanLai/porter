//! syncd: the sync daemon (`org.quire.Sync1`, design/31 §4.1, §6).
//!
//! It resolves its paths from the environment, loads the caller tables (the files accountd
//! reads), serves `org.quire.Sync1` over the hub of running datasets, and listens for accountd's
//! `AccountRemoved` to wipe an account's journals and mirrors. The PIM supervisor (W6e) keeps a
//! calendar or address book mirror running for every Calendar or Contacts grant syncd holds;
//! with no grant `Datasets` answers an empty list and every name the refusal
//! `NoFittingAccount`.

use clap::Parser;
use porter_client::{Accounts, DbusTransport};
use porter_dbus::ProcCallers;
use std::process::ExitCode;
use std::sync::Arc;
use syncd::datasets::pim::{ClientGrants, PimConfig, PimSupervisor, Wiring};
use syncd::paths::{BUILD, Paths, proc_root};
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access, Hub};
use syncd::{callers_file, removal, service};

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
    let paths = match Paths::resolve(|name| std::env::var(name).ok()) {
        Ok(paths) => paths,
        Err(why) => return fail(why),
    };
    let table = match callers_file::load_callers(&paths.callers_system, &paths.callers_user) {
        Ok(table) => table,
        Err(why) => return fail(why),
    };
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(why) => return fail(format!("no session bus: {why}")),
    };
    let callers = Arc::new(
        match proc_root(BUILD, std::env::var("SYNCD_PROC_ROOT").ok()) {
            Some(root) => {
                eprintln!(
                    "syncd: reading callers from {} (test-proc-root build)",
                    root.display()
                );
                ProcCallers::with_proc_root(connection.clone(), table, root)
            }
            None => ProcCallers::new(connection.clone(), table),
        },
    );
    let hub = Hub::default();
    // NetworkManager is not read yet: the network is taken as unmetered and up.
    let (_network_keeps, network) = tokio::sync::watch::channel(Network::Unmetered);
    let accounts = Arc::new(Accounts::over(DbusTransport::over(connection.clone())));
    let wiring = Wiring {
        accounts: Arc::clone(&accounts),
        hub: hub.clone(),
        paths: paths.clone(),
        settings: Settings::default(),
        network,
        owners: Access::default(),
    };
    let _supervisor =
        PimSupervisor::new(wiring, ClientGrants::new(accounts), PimConfig::default()).spawn();
    if let Err(why) = removal::watch(&connection, hub.clone(), paths).await {
        return fail(format!("cannot listen for AccountRemoved: {why}"));
    }
    if let Err(why) = service::serve(&connection, hub, callers).await {
        return fail(format!("cannot serve {}: {why}", porter_dbus::SYNC_BUS));
    }
    // Serves until the session ends or the unit stops it (SIGTERM ends the process).
    std::future::pending::<()>().await;
    ExitCode::SUCCESS
}
