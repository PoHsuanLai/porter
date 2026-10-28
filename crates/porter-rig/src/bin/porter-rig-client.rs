//! `porter-rig-client --app-id <id> [--proc-root <dir>] <command>`: acts as an app over the
//! session bus it is given (`DBUS_SESSION_BUS_ADDRESS`) and prints JSON lines.
//!
//! Commands:
//! - `identities list`: the identities signed in on this machine (`{"result":"ok","identities":[..]}`).
//! - `identities offer [<provider>]`: "Continue as": the person picks in a sheet
//!   (`{"result":"picked","identity":..,"address":..}`, or `refused` / `none`).
//! - `identities register-use <identity>`: says this app signed in with it.
//! - `ai-keys request [--class C] [--usage U]`: asks inferd for a permit to use the person's AI
//!   keys (`{"result":"granted","permit":..}` or `refused`).
//! - `ai-keys add [<provider>]`: inferd's add-a-key sheet (`{"result":"added","account":..}`).
//! - `launcher request-key <program> [--class C] [--session S]`: as the agent launcher, asks which
//!   key the agent may use (`{"result":"granted","permit":..,"account":..}`).
//! - `launcher issue <permit> <program> [--target memfd|tmpfs_file]`: as the agent launcher, asks
//!   for a process credential (`{"result":"issued","id":..}`; the key is never printed).
//! - `open <need> [--class C] [--tier T] [--usage U] [--prompt TEXT]`: opens an AI session
//!   (`{"result":"opened"}`) and, with a prompt, runs one chat turn, one `{"event":..}` line
//!   per event.
//! - `sync-resolve <dataset> <number> <keep_local|keep_remote>`.
//! - `watch-conflicts [--count N] [--timeout-s S]`: `{"event":"conflict",..}` lines.
//! - `add-account [<provider>]`: opens accountd's add-account sheet (the provider's form when
//!   named) and waits for the answer: `{"result":"added","account":..}`, or
//!   `{"result":"refused","refusal":"Cancelled"|"Unavailable"|..}` and exit 1.
//! - `reauthenticate <account>`: the same for signing an account in again.
//! - `datasets`, `status <dataset>`.
//! - `write-identity --pid N`: only writes the identity fixture for process N (it is kept).
//!
//! `<need>` is `mail`, `storage`, `calendar`, `contacts`, `notes`, `llm` or a JSON `Need`;
//! `C` and `U` and `T` are the wire words (`files`, `photos`, `mail`, `public`, ...;
//! `interactive`, `background`; `fast`, `balanced`, `best`). Exit status 0 when the command
//! did what it was asked, 1 otherwise.
//!
//! Identity: see `porter_rig::fixture`. Test tooling, never installed.

use clap::{Parser, Subcommand};
use porter_core::audit::Handoff;
use porter_core::capability::AgentProgram;
use porter_core::consent::Usage;
use porter_core::{AccountId, KeyPermitId, ProviderId};
use porter_core::{DataClass, IdentityId, LauncherSession};
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
    /// The identities signed in on this machine, and "Continue as".
    #[command(subcommand)]
    Identities(IdentitiesSub),
    /// The person's AI keys: ask for a permit to use them, or add one.
    #[command(subcommand)]
    AiKeys(AiKeysSub),
    /// The agent launcher's calls to inferd.
    #[command(subcommand)]
    Launcher(LauncherSub),
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
    WriteIdentity {
        #[arg(long)]
        pid: u32,
    },
}

#[derive(Debug, Subcommand)]
enum IdentitiesSub {
    List,
    Offer {
        /// The provider's file id; any when absent.
        provider: Option<String>,
    },
    RegisterUse {
        identity: String,
    },
}

#[derive(Debug, Subcommand)]
enum AiKeysSub {
    Request {
        #[arg(long, default_value = "files")]
        class: String,
        #[arg(long, default_value = "interactive")]
        usage: String,
    },
    Add {
        /// The provider's file id; the list when absent.
        provider: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum LauncherSub {
    RequestKey {
        program: String,
        #[arg(long, default_value = "files")]
        class: String,
        #[arg(long)]
        session: Option<String>,
    },
    Issue {
        permit: String,
        program: String,
        #[arg(long, default_value = "tmpfs_file")]
        target: String,
    },
}

fn provider(id: &str) -> Result<ProviderId, String> {
    ProviderId::parse(id).map_err(|e| format!("`{id}` is not a provider: {e}"))
}

fn program(name: &str) -> Result<AgentProgram, String> {
    AgentProgram::parse(name).map_err(|e| format!("`{name}` is not an agent program: {e}"))
}

fn identities(sub: IdentitiesSub) -> Result<Command, String> {
    Ok(match sub {
        IdentitiesSub::List => Command::IdentitiesList,
        IdentitiesSub::Offer { provider: id } => Command::IdentitiesOffer {
            provider: id.as_deref().map(provider).transpose()?,
        },
        IdentitiesSub::RegisterUse { identity } => Command::IdentitiesRegisterUse {
            identity: IdentityId::parse(&identity)
                .map_err(|e| format!("`{identity}` is not an identity id: {e}"))?,
        },
    })
}

fn ai_keys(sub: AiKeysSub) -> Result<Command, String> {
    Ok(match sub {
        AiKeysSub::Request { class, usage } => Command::AiKeysRequest {
            ask: ask(&class, &usage)?,
        },
        AiKeysSub::Add { provider: id } => Command::AiKeysAdd {
            provider: id.as_deref().map(provider).transpose()?,
        },
    })
}

fn launcher(sub: LauncherSub) -> Result<Command, String> {
    Ok(match sub {
        LauncherSub::RequestKey {
            program: name,
            class,
            session,
        } => Command::LauncherRequestKey {
            program: program(&name)?,
            class: word::<DataClass>(&class)?,
            session: session
                .map(|text| {
                    LauncherSession::parse(&text)
                        .map_err(|e| format!("`{text}` is not a session: {e}"))
                })
                .transpose()?,
        },
        LauncherSub::Issue {
            permit,
            program: name,
            target,
        } => Command::LauncherIssue {
            permit: KeyPermitId::parse(&permit)
                .map_err(|e| format!("`{permit}` is not a permit id: {e}"))?,
            program: program(&name)?,
            target: match target.as_str() {
                "memfd" => Handoff::Memfd,
                "tmpfs_file" => Handoff::TmpfsFile,
                other => return Err(format!("`{other}` is not memfd or tmpfs_file")),
            },
        },
    })
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
        Sub::Identities(sub) => identities(sub)?,
        Sub::AiKeys(sub) => ai_keys(sub)?,
        Sub::Launcher(sub) => launcher(sub)?,
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
