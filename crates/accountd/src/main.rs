//! accountd: the account registry, consent store and token broker on the session bus
//! (design/31 §4.1).
//!
//! It resolves its paths from the environment, loads the caller tables, the provider files and
//! the registry (refusing to start over a registry it cannot read), builds the service over its
//! seams (the Secret Service through oo7, the sheet host over `org.quire.AccountsSheet1`, the
//! families, the file store and audit) and serves `org.quire.Accounts1`.

mod clock;

use accountd::paths::{BUILD, Paths, proc_root};
use accountd::{
    AdoptConfig, AdoptTable, BusSheets, FileAudit, FileStore, Oo7Legacy, Options, RelayRoots,
    load_callers, serve_with,
};
use clap::Parser;
use clock::SystemClock;
use porter_dbus::ProcCallers;
use porter_secrets::Oo7Secrets;
use porter_service::{AccountService, Registry, RegistryStore};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

/// porter's account and consent service (`org.quire.Accounts1`).
#[derive(Debug, Parser)]
#[command(name = "accountd", version)]
struct Args {
    /// A directory of provider files, in addition to the system and user ones; later
    /// directories win.
    #[arg(long = "providers", value_name = "DIR")]
    provider_dirs: Vec<PathBuf>,
}

/// The daemon's one log path is standard error, prefixed with its name.
fn fail(why: impl std::fmt::Display) -> ExitCode {
    eprintln!("accountd: {why}");
    ExitCode::FAILURE
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    let paths = match Paths::resolve(|name| std::env::var(name).ok(), &args.provider_dirs) {
        Ok(paths) => paths,
        Err(why) => return fail(why),
    };
    let table = match load_callers(&paths.callers_system, &paths.callers_user) {
        Ok(table) => table,
        Err(why) => return fail(why),
    };
    let adopt = match std::fs::read_to_string(&paths.config) {
        Ok(text) => match AdoptTable::from_config(&text) {
            Ok(table) => table,
            Err(why) => return fail(format!("{}: {why}", paths.config.display())),
        },
        Err(_) => AdoptTable::default(),
    };

    let loaded = accountd::providers::load_specs(&paths.provider_dirs);
    for (file, why) in &loaded.skipped {
        eprintln!("accountd: skipped provider file {}: {why}", file.display());
    }
    let io = accountd::providers::FamilyIo::system(porter_provider::ProviderSet::layered(
        loaded.specs.clone(),
        Vec::new(),
    ));
    let (families, unserved) = accountd::providers::served(loaded.specs, &io);
    for spec in &unserved {
        eprintln!("accountd: no family serves provider `{}` yet", spec.id);
    }

    let store = FileStore::new(paths.registry_dir.clone());
    let registry = match store.load().await {
        Ok(stored) => Registry::from_persisted(stored),
        Err(why) => return fail(format!("{}: {why}", store.path().display())),
    };

    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(why) => return fail(format!("no session bus: {why}")),
    };
    let callers = Arc::new(
        match proc_root(BUILD, std::env::var("ACCOUNTD_PROC_ROOT").ok()) {
            Some(root) => {
                eprintln!(
                    "accountd: reading callers from {} (test-proc-root build)",
                    root.display()
                );
                ProcCallers::with_proc_root(connection.clone(), table, root)
            }
            None => ProcCallers::new(connection.clone(), table),
        },
    );
    let sheets = BusSheets::new(connection.clone(), Arc::clone(&callers));
    let service = Arc::new(
        AccountService::new(families, registry, Oo7Secrets, sheets, SystemClock)
            .with_store(store)
            .with_audit(FileAudit::new(paths.audit.clone())),
    );
    let options = Options {
        adopt: AdoptConfig {
            table: adopt,
            store: Some(Arc::new(Oo7Legacy)),
        },
        clients: Some(paths.clients_user.clone()),
        relay_roots: RelayRoots::Platform,
    };
    if let Err(why) = serve_with(&connection, service, callers, options).await {
        return fail(format!("cannot serve {}: {why}", porter_dbus::ACCOUNTS_BUS));
    }
    // Serves until the session ends or the unit stops it (SIGTERM ends the process).
    std::future::pending::<()>().await;
    ExitCode::SUCCESS
}
