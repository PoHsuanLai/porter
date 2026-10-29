//! `porter-rig-client --app-id <id> [--proc-root <dir>] <command>`: acts as an app over the
//! session bus it is given (`DBUS_SESSION_BUS_ADDRESS`) and prints JSON lines.
//!
//! Commands:
//! - `identities list`: the identities signed in on this machine (`{"result":"ok","identities":[..]}`).
//! - `identities offer [<provider>]`: "Continue as": the person picks in a sheet
//!   (`{"result":"picked","identity":..,"address":..}`, or `refused` / `none`).
//! - `identities register-use <identity>`: says this app signed in with it.
//! - `ai-keys request [--class C] [--usage U]`: asks accountd for a permit to use the person's AI
//!   keys (`{"result":"granted","permit":..}` or `refused`); adding a key is `add-account`.
//! - `launcher request-key <program> [--class C] [--session S]`: as the agent launcher, asks which
//!   key the agent may use (`{"result":"granted","permit":..,"account":..}`).
//! - `launcher issue <permit> <program>`: as the agent launcher, asks for a process credential on a
//!   memfd (`{"result":"issued","id":..}`; the key is never printed).
//! - `open <need> [--class C] [--tier T] [--usage U] [--prompt TEXT]`: opens an AI session
//!   (`{"result":"opened"}`) and, with a prompt, runs one chat turn, one `{"event":..}` line
//!   per event.
//! - `add-account [<provider>]`: opens accountd's add-account sheet (the provider's form when
//!   named) and waits for the answer: `{"result":"added","account":..}`, or
//!   `{"result":"refused","refusal":"Cancelled"|"Unavailable"|..}` and exit 1.
//! - `reauthenticate <account>`: the same for signing an account in again.
//! - `write-identity --pid N`: only writes the identity fixture for process N (it is kept).
//!
//! `<need>` is `mail`, `storage`, `calendar`, `contacts`, `notes`, `llm` or a JSON `Need`;
//! `C` and `U` and `T` are the wire words (`files`, `photos`, `mail`, `public`, ...;
//! `interactive`, `background`; `fast`, `balanced`, `best`). Exit status 0 when the command
//! did what it was asked, 1 otherwise.
//!
//! Identity: see `porter_rig::fixture`. Test tooling, never installed.

use clap::Parser;
use clap::error::ErrorKind;
use porter_rig::client::{Command, run};
use porter_rig::fixture::Identity;
use std::path::PathBuf;
use std::process::ExitCode;

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
    command: Command,
}

fn fail(why: impl std::fmt::Display) -> ExitCode {
    println!(
        "{}",
        serde_json::json!({ "result": "error", "error": why.to_string() })
    );
    ExitCode::FAILURE
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut args = match Args::try_parse() {
        Ok(args) => args,
        // A word or id that is none is an error line like any other failure.
        Err(why) if why.kind() == ErrorKind::ValueValidation => return fail(why.render()),
        Err(why) => why.exit(),
    };
    args.proc_root = args
        .proc_root
        .or_else(|| std::env::var_os("RIG_PROC_ROOT").map(PathBuf::from));
    if let Command::WriteIdentity { pid } = &args.command {
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
    let ok = run(&connection, args.command, &mut |line| println!("{line}")).await;
    match ok {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}
