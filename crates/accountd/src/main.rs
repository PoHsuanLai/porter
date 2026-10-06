//! accountd: the account registry, consent store and token broker on the session bus
//! (design/31 §4.1).
//!
//! It resolves its paths from the environment, loads the caller tables, the provider files and
//! the registry (refusing to start over a registry it cannot read), builds the service over its
//! seams (the Secret Service through oo7, the sheet host over `org.quire.AccountsSheet1`, the
//! families, the file store and audit) and serves `org.quire.Accounts1`.

mod clock;

use accountd::add::{AddArgs, StdTerminal, TerminalSheets};
use accountd::paths::{BUILD, Paths, proc_root};
use accountd::{
    AdoptConfig, AdoptTable, BusSheets, FileAudit, FileStore, Oo7Legacy, Options, RelayRoots,
    SecretsDesk, load_callers, serve_with,
};
use clap::{Parser, Subcommand};
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
    #[arg(long = "providers", value_name = "DIR", global = true)]
    provider_dirs: Vec<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add an account from this terminal, for a computer with no sheet host yet
    ///
    /// Adds an account for PROVIDER, an AI company's provider file (`anthropic`, `google-ai`,
    /// `moonshot`, `openai`, `openrouter`): you paste the API key you got from the company, it is checked with
    /// one authenticated call to the company (its models list; OpenRouter's key call), and the account is filed in the same secret store
    /// and registry the daemon uses. The key is read with echo off and is never printed or logged.
    ///
    /// Each --allow gives that app the new account for language models: it records the grant an
    /// "Allow, always" answer to the app's consent sheet would. You typing this command is the
    /// consent. The grant covers every data class unless you narrow it with --class, and
    /// interactive use only; which classes may leave this computer is still decided by the
    /// ai.floor.<class> settings.
    ///
    /// accountd reads its registry only when it starts, so this command takes the bus name
    /// org.quire.Accounts1 while it runs and stops if a daemon already owns it: stop the unit
    /// (systemctl --user stop accountd.service), run this, then start the unit again.
    Add {
        /// The provider file's id, such as `anthropic` or `openai`
        provider: String,
        /// An app to give the account to: `org.quire.Companion`, or `name:flatpak` for a
        /// sandboxed one (a native app is the default). May be repeated
        #[arg(long = "allow", value_name = "APP-ID")]
        allow: Vec<String>,
        /// Narrow --allow to this data class (`prompt`, `notes`, ...). May be repeated
        #[arg(long = "class", value_name = "CLASS")]
        classes: Vec<String>,
    },
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
    if let Some(Command::Add {
        provider,
        allow,
        classes,
    }) = &args.command
    {
        return add(&paths, provider, allow, classes).await;
    }
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
    let keys = SecretsDesk::new(Oo7Secrets, FileAudit::new(paths.audit.clone()), SystemClock);
    let options = Options {
        adopt: AdoptConfig {
            table: adopt,
            store: Some(Arc::new(Oo7Legacy)),
        },
        clients: Some(paths.clients_user.clone()),
        relay_roots: RelayRoots::Platform,
        keys: Some(Arc::new(keys)),
    };
    if let Err(why) = serve_with(&connection, service, callers, options).await {
        return fail(format!("cannot serve {}: {why}", porter_dbus::ACCOUNTS_BUS));
    }
    // Serves until the session ends or the unit stops it (SIGTERM ends the process).
    std::future::pending::<()>().await;
    ExitCode::SUCCESS
}

/// `accountd add`: the provider files, the registry and the secret store the daemon uses, a
/// terminal for a sheet, and the bus name as the lock.
async fn add(paths: &Paths, provider: &str, allow: &[String], classes: &[String]) -> ExitCode {
    let args = match AddArgs::parse(provider, allow, classes) {
        Ok(args) => args,
        Err(why) => return fail(why),
    };
    let loaded = accountd::providers::load_specs(&paths.provider_dirs);
    for (file, why) in &loaded.skipped {
        eprintln!("accountd: skipped provider file {}: {why}", file.display());
    }
    let io = accountd::providers::FamilyIo::system(porter_provider::ProviderSet::layered(
        loaded.specs.clone(),
        Vec::new(),
    ));
    let (families, _unserved) = accountd::providers::served(loaded.specs, &io);
    let served: Vec<_> = families
        .iter()
        .map(|family| porter_provider::Provider::spec(family).id.clone())
        .collect();

    // The lock. With no session bus no daemon can be reached either, so go on and say so.
    let session = zbus::Connection::session().await.ok();
    match &session {
        Some(connection) => {
            if let Err(why) = accountd::add::take_the_name(connection).await {
                return fail(why);
            }
        }
        None => eprintln!("accountd: no session bus; cannot tell whether the daemon is running"),
    }

    let store = FileStore::new(paths.registry_dir.clone());
    let registry = match store.load().await {
        Ok(stored) => Registry::from_persisted(stored),
        Err(why) => return fail(format!("{}: {why}", store.path().display())),
    };
    let sheets = TerminalSheets::new(StdTerminal);
    let service = AccountService::new(families, registry, Oo7Secrets, sheets.clone(), SystemClock)
        .with_store(store)
        .with_audit(FileAudit::new(paths.audit.clone()));
    match accountd::add::run(&service, &sheets, &served, &args).await {
        Ok(report) => {
            println!("Added account {}.", report.account.as_str());
            for (app, class, grant) in &report.grants {
                println!(
                    "Allowed {} for {class:?} (grant {}).",
                    app.name.as_str(),
                    grant.as_str()
                );
            }
            ExitCode::SUCCESS
        }
        Err(why) => fail(why),
    }
}
