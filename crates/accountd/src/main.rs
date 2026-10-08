//! accountd: the account registry, consent store and token broker on the session bus
//! (design/31 §4.1).
//!
//! It resolves its paths from the environment, loads the caller tables, the provider files and
//! the registry (refusing to start over a registry it cannot read), builds the service over its
//! seams (the Secret Service through oo7, or in a `test-keys` build the key file `ACCOUNTD_KEYS`
//! names, the sheet host over `org.quire.AccountsSheet1`, the
//! families, the file store and audit) and serves `org.quire.Accounts1`.

mod clock;

use accountd::add::{AddArgs, StdTerminal, TerminalSheets};
use accountd::keysel::{AnyKeys, Chosen};
use accountd::paths::{BUILD, Paths, proc_root};
use accountd::{
    AppNames, BusSheets, FileAudit, FileStore, Options, ProviderNames, RelayRoots, SecretsDesk,
    load_callers, serve_with,
};
use clap::{Parser, Subcommand};
use clock::SystemClock;
use porter_dbus::ProcCallers;
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
    // Which secret store, before anything reads or files a credential: refused here, not later.
    let chosen = match Chosen::from_env() {
        Ok(chosen) => chosen,
        Err(why) => return fail(why),
    };
    eprintln!("accountd: {}", chosen.said);
    let secrets = chosen.keys;
    if let Some(Command::Add {
        provider,
        allow,
        classes,
    }) = &args.command
    {
        return add(&paths, secrets, provider, allow, classes).await;
    }
    let table = match load_callers(&paths.callers_system, &paths.callers_user) {
        Ok(table) => table,
        Err(why) => return fail(why),
    };

    let loaded = accountd::providers::load_specs(&paths.provider_layers());
    for (file, why) in &loaded.skipped {
        eprintln!("accountd: skipped provider file {}: {why}", file.display());
    }
    // The families read the clients files themselves (porter-oauth's registry); this names a row
    // of the person's own that tried to send a sign-in elsewhere (sec-4), whose endpoints the
    // registry sets aside.
    let own_clients = std::fs::read_to_string(&paths.clients_user)
        .ok()
        .and_then(|text| porter_provider::parse_clients(&text).ok())
        .unwrap_or_default();
    for row in own_clients.clients.iter().filter(|c| c.leaves_the_issuer()) {
        eprintln!(
            "accountd: {}: the {:?} client's own sign-in addresses are not the issuer's; \
             the issuer's own are used",
            paths.clients_user.display(),
            row.issuer
        );
    }
    let app_names =
        AppNames::new(table.clone(), paths.applications.clone()).with_agents(&loaded.specs);
    let provider_names = ProviderNames::from_specs(&loaded.specs);
    let io = accountd::providers::FamilyIo::system(porter_provider::ProviderSet::layered(
        loaded.specs.clone(),
        Vec::new(),
    ));
    let (families, unserved) = accountd::providers::served(loaded.specs, &io);
    let local = accountd::providers::local_runtimes(&unserved);
    for spec in unserved.iter().filter(|s| !local.contains(s)) {
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
    let sheets =
        BusSheets::new(connection.clone(), Arc::clone(&callers)).with_names(app_names.clone());
    let service = Arc::new(
        AccountService::new(families, registry, secrets.clone(), sheets, SystemClock)
            .with_local_runtimes(local)
            .with_store(store)
            .with_audit(FileAudit::new(paths.audit.clone())),
    );
    let keys = SecretsDesk::new(secrets, FileAudit::new(paths.audit.clone()), SystemClock);
    let options = Options {
        clients: Some(paths.clients_user.clone()),
        relay_roots: RelayRoots::Platform,
        keys: Some(Arc::new(keys)),
        app_names,
        provider_names,
        login: accountd::LoginTiming {
            audit: Some(Arc::new(FileAudit::new(paths.audit.clone()))),
            ..accountd::LoginTiming::default()
        },
        // The XDG rule: a relative path is invalid and ignored.
        runtime_dir: std::env::var("XDG_RUNTIME_DIR")
            .ok()
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute()),
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
async fn add(
    paths: &Paths,
    secrets: AnyKeys,
    provider: &str,
    allow: &[String],
    classes: &[String],
) -> ExitCode {
    let args = match AddArgs::parse(provider, allow, classes) {
        Ok(args) => args,
        Err(why) => return fail(why),
    };
    let loaded = accountd::providers::load_specs(&paths.provider_layers());
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
    let service = AccountService::new(families, registry, secrets, sheets.clone(), SystemClock)
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
