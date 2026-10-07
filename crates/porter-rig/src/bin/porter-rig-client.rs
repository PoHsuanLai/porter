//! `porter-rig-client --app-id <id> [--proc-root <dir>] <command>`: acts as an app over the
//! session bus it is given (`DBUS_SESSION_BUS_ADDRESS`) and prints JSON lines.
//!
//! Commands:
//! - `request-grant <need> [--class C] [--usage U]`: finds an account, asks for a grant when
//!   the app has none; `{"result":"granted"|"several"|"refused"|"none"|"error",..}`.
//! - `open <need> [--class C] [--tier T] [--usage U] [--prompt TEXT]`: opens an AI session
//!   (`{"result":"opened"}`) and, with a prompt, runs one chat turn, one `{"event":..}` line
//!   per event.
//! - `open-authenticated <need> [--class C] [--usage U] [--family F] [--path P]`: opens the
//!   relay and prints what comes through it (`said`: the IMAP/SMTP/POP3 greeting, or the HTTP
//!   status of a `GET P`).
//! - `sync-resolve <dataset> <number> <keep_local|keep_remote>`.
//! - `watch-conflicts [--count N] [--timeout-s S]`: `{"event":"conflict",..}` lines.
//! - `add-account [<provider>]`: opens accountd's add-account sheet (the provider's form when
//!   named) and waits for the answer: `{"result":"added","account":..}`, or
//!   `{"result":"refused","refusal":"Cancelled"|"Unavailable"|..}` and exit 1.
//! - `reauthenticate <account>`: the same for signing an account in again.
//! - `datasets`, `status <dataset>`, `grants`.
//! - `write-identity --pid N`: only writes the identity fixture for process N (it is kept).
//!
//! `<need>` is `mail`, `storage`, `calendar`, `contacts`, `notes`, `llm` or a JSON `Need`;
//! `C` and `U` and `T` are the wire words (`files`, `photos`, `mail`, `public`, ...;
//! `interactive`, `background`; `fast`, `balanced`, `best`). Exit status 0 when the command
//! did what it was asked, 1 otherwise.
//!
//! Identity: see `porter_rig::fixture`. Test tooling, never installed.

use clap::{Parser, Subcommand};
use porter_core::consent::Usage;
use porter_core::{AccountId, DataClass, ProviderId};
use porter_rig::client::{Ask, Command, parse_need, run, word};
use porter_rig::fixture::Identity;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// An app, for a jailed scenario.
#[derive(Debug, Parser)]
#[command(name = "porter-rig-client", version)]
struct Args {
    /// The app this process is (`org.example.App`).
    #[arg(long)]
    app_id: String,
    /// Where the daemons read callers from (`ACCOUNTD_PROC_ROOT`): this process is named there.
    /// Defaults to `$RIG_PROC_ROOT`.
    #[arg(long)]
    proc_root: Option<PathBuf>,
    #[command(subcommand)]
    command: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    RequestGrant {
        need: String,
        #[arg(long, default_value = "files")]
        class: String,
        #[arg(long, default_value = "interactive")]
        usage: String,
    },
    Open {
        need: String,
        #[arg(long, default_value = "public")]
        class: String,
        #[arg(long, default_value = "fast")]
        tier: String,
        #[arg(long, default_value = "interactive")]
        usage: String,
        #[arg(long)]
        prompt: Option<String>,
    },
    OpenAuthenticated {
        need: String,
        #[arg(long, default_value = "files")]
        class: String,
        #[arg(long, default_value = "interactive")]
        usage: String,
        #[arg(long)]
        family: Option<String>,
        #[arg(long, default_value = "/")]
        path: String,
    },
    SyncResolve {
        dataset: String,
        number: i64,
        how: String,
    },
    WatchConflicts {
        #[arg(long)]
        count: Option<usize>,
        #[arg(long)]
        timeout_s: Option<u64>,
    },
    AddAccount {
        /// The provider's file id (`microsoft`, `nextcloud`, ...); the list when absent.
        provider: Option<String>,
    },
    Reauthenticate {
        account: String,
    },
    Datasets,
    Status {
        dataset: String,
    },
    Grants,
    WriteIdentity {
        #[arg(long)]
        pid: u32,
    },
}

fn fail(why: impl std::fmt::Display) -> ExitCode {
    println!(
        "{}",
        serde_json::json!({ "result": "error", "error": why.to_string() })
    );
    ExitCode::FAILURE
}

fn ask(class: &str, usage: &str) -> Result<Ask, String> {
    Ok(Ask {
        class: word::<DataClass>(class)?,
        usage: word::<Usage>(usage)?,
    })
}

fn command(sub: Sub) -> Result<Command, String> {
    Ok(match sub {
        Sub::RequestGrant { need, class, usage } => Command::RequestGrant {
            need: parse_need(&need)?,
            ask: ask(&class, &usage)?,
        },
        Sub::Open {
            need,
            class,
            tier,
            usage,
            prompt,
        } => Command::Open {
            need: parse_need(&need)?,
            ask: ask(&class, &usage)?,
            tier: word(&tier)?,
            prompt,
        },
        Sub::OpenAuthenticated {
            need,
            class,
            usage,
            family,
            path,
        } => Command::OpenAuthenticated {
            need: parse_need(&need)?,
            ask: ask(&class, &usage)?,
            family,
            path,
        },
        Sub::SyncResolve {
            dataset,
            number,
            how,
        } => Command::SyncResolve {
            dataset,
            number,
            how,
        },
        Sub::WatchConflicts { count, timeout_s } => Command::WatchConflicts {
            count,
            timeout: timeout_s.map(Duration::from_secs),
        },
        Sub::AddAccount { provider } => Command::AddAccount {
            provider: provider
                .map(|id| {
                    ProviderId::parse(&id).map_err(|e| format!("`{id}` is not a provider: {e}"))
                })
                .transpose()?,
        },
        Sub::Reauthenticate { account } => Command::Reauthenticate {
            account: AccountId::parse(&account)
                .map_err(|e| format!("`{account}` is not an account id: {e}"))?,
        },
        Sub::Datasets => Command::Datasets,
        Sub::Status { dataset } => Command::Status { dataset },
        Sub::Grants => Command::Grants,
        Sub::WriteIdentity { .. } => return Err("write-identity is not a bus command".to_owned()),
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut args = Args::parse();
    args.proc_root = args
        .proc_root
        .or_else(|| std::env::var_os("RIG_PROC_ROOT").map(PathBuf::from));
    if let Sub::WriteIdentity { pid } = &args.command {
        let Some(root) = &args.proc_root else {
            return fail("write-identity needs --proc-root");
        };
        return match Identity::write(root, &args.app_id, *pid) {
            Ok(identity) => {
                identity.keep();
                println!("{}", serde_json::json!({ "result": "ok", "pid": pid }));
                ExitCode::SUCCESS
            }
            Err(why) => fail(format!("cannot write the identity: {why}")),
        };
    }
    let command = match command(args.command) {
        Ok(command) => command,
        Err(why) => return fail(why),
    };
    // Named before the first call, removed when the command ends.
    let _identity = match &args.proc_root {
        Some(root) => match Identity::write(root, &args.app_id, std::process::id()) {
            Ok(identity) => Some(identity),
            Err(why) => return fail(format!("cannot write the identity: {why}")),
        },
        None => None,
    };
    let connection = match zbus::Connection::session().await {
        Ok(connection) => connection,
        Err(why) => return fail(format!("no session bus: {why}")),
    };
    let ok = run(&connection, command, &mut |line| println!("{line}")).await;
    match ok {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}
