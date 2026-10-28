//! accountd: the identities signed in on this computer, Spaces and Tailscale, on the session bus
//! (design/31 §4.1).
//!
//! The binary reads its arguments, asks `Config::from_env` for the environment's answers, then
//! builds and runs the daemon, all in the library (`accountd::daemon`): it resolves its paths,
//! loads the caller tables, the provider files and the registry (refusing to start over a
//! registry it cannot read), builds the service over its seams (the Secret Service through oo7 in
//! the `porter.accountd` namespace, or in a `test-keys` build the key file `ACCOUNTD_KEYS` names,
//! the sheet host over `org.quire.AccountsSheet1`, the families, the file store and audit) and
//! serves `org.quire.Accounts1`. `accountd add` adds an AI key from a terminal.

// The daemon's future holds the service's, whose domain traits each wrap their async bodies.
#![recursion_limit = "256"]

use accountd::daemon::{EXIT_RESTART_FOR_KEYS, RunEnd};
use accountd::{Config, Daemon, StartError};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

/// porter's identity service (`org.quire.Accounts1`).
#[derive(Debug, Parser)]
#[command(name = "accountd", version)]
struct Args {
    /// A directory of provider files, in addition to the system and user ones; later
    /// directories win.
    #[arg(long = "providers", value_name = "DIR")]
    provider_dirs: Vec<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add an AI key from this terminal, for a computer with no sheet host yet
    ///
    /// Adds an account for PROVIDER, an AI company's provider file (`anthropic`, `google-ai`,
    /// `moonshot`, `openai`, `openrouter`): you paste the API key you got from the company, it
    /// is checked with one call to the company (its models list), and the key is filed in the
    /// secret store accountd uses. The key is read with echo off and is never printed or logged.
    ///
    /// Each --allow gives that app the new account for language models: it records the
    /// permission an "Allow, always" answer to the app's consent sheet would. You typing this
    /// command is the consent. It covers every data class unless you narrow it with --class,
    /// and interactive use only; which classes may leave this computer is still decided by the
    /// ai.floor.<class> settings.
    ///
    /// accountd reads its accounts only when it starts, so this command takes the bus name
    /// org.quire.Accounts1 while it runs and stops if the daemon already owns it: stop the unit
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
fn fail(why: impl std::fmt::Display, code: ExitCode) -> ExitCode {
    eprintln!("accountd: {why}");
    code
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    let config = match Config::from_env(&args.provider_dirs) {
        Ok(config) => config,
        Err(why) => return fail(&why, why.exit_code()),
    };
    if let Some(Command::Add {
        provider,
        allow,
        classes,
    }) = &args.command
    {
        return match accountd::add::add_account(config, provider, allow, classes).await {
            Ok(report) => {
                println!("Added account {}.", report.account.as_str());
                for (app, class, permit) in &report.permits {
                    println!(
                        "Allowed {} for {class:?} (permission {}).",
                        app.name.as_str(),
                        permit.as_str()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(why) => fail(why, ExitCode::FAILURE),
        };
    }
    let daemon = match Daemon::build(config).await {
        Ok(daemon) => daemon,
        Err(why) => return fail(&why, StartError::exit_code(&why)),
    };
    // Serves until the session ends or the unit stops it (SIGTERM ends the process).
    match daemon.run(std::future::pending()).await {
        RunEnd::KeysReady => {
            eprintln!(
                "accountd: the inferd that kept the AI keys is gone; starting again to copy them"
            );
            ExitCode::from(EXIT_RESTART_FOR_KEYS)
        }
        _ => ExitCode::SUCCESS,
    }
}
