//! syncd: the sync daemon (`org.quire.Sync1`, design/31 §4.1, §6).
//!
//! It resolves its paths from the environment, loads the caller tables (the files accountd
//! reads), serves `org.quire.Sync1` over the hub of running datasets, and listens for accountd's
//! `AccountRemoved` to wipe an account's journals and mirrors. The PIM supervisor (W6e) keeps a
//! calendar or address book mirror running for every Calendar or Contacts grant syncd holds;
//! with no grant `Datasets` answers an empty list and every name the refusal
//! `NoFittingAccount`. The storage supervisor keeps an app folder mirror running for every
//! Storage grant (class Files) on a Microsoft account and, with `SYNCD_PHOTOS=on`, the Photos
//! datasets for every one of class Photos. A test build (`test-proc-root`) reads
//! `SYNCD_RESCAN_S` as how often both supervisors read grants again; the default is ten minutes.

use clap::Parser;
use porter_client::{Accounts, DbusTransport};
use porter_dbus::ProcCallers;
use std::process::ExitCode;
use std::sync::Arc;
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::pim::{ClientGrants, PimConfig, PimSupervisor, Wiring};
use syncd::datasets::storage::{ClientStorageGrants, StorageConfig, StorageSupervisor};
use syncd::paths::{BUILD, Paths, proc_root, rescan};
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
    let wiring = |network| Wiring {
        accounts: Arc::clone(&accounts),
        hub: hub.clone(),
        paths: paths.clone(),
        settings: Settings::default(),
        network,
        owners: Access::default(),
    };
    // `SYNCD_RESCAN_S` shortens how often grants are read again, in a test build only.
    let rescan_var = std::env::var("SYNCD_RESCAN_S").ok();
    let pim = PimConfig::default();
    let _pim = PimSupervisor::new(
        wiring(network.clone()),
        ClientGrants::new(Arc::clone(&accounts)),
        PimConfig {
            rescan: rescan(BUILD, rescan_var.clone(), pim.rescan),
        },
    )
    .spawn();
    // Storage over Graph: the app folder mirror for a Files grant, and Photos for a Photos grant
    // when `SYNCD_PHOTOS=on` (it is off otherwise: there is no Photos app yet).
    let storage = StorageConfig::default();
    let storage = StorageConfig {
        rescan: rescan(BUILD, rescan_var, storage.rescan),
        photos: PhotosSwitch::from_var(std::env::var("SYNCD_PHOTOS").ok().as_deref()),
        ..StorageConfig::default()
    };
    let _storage = StorageSupervisor::new(
        wiring(network),
        ClientStorageGrants::new(Arc::clone(&accounts)),
        storage,
    )
    .spawn();
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
