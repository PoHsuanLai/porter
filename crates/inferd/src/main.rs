//! inferd: the AI broker (design/31 §4.1, §5.5): serves `org.quire.Inference1` on the session
//! bus. It reads `inferd.toml` (engine programs, the `ai.*` settings rows, the caller table), the stoker
//! catalog, and runs engines as child processes of its own (the systemd transient-unit host is
//! not built). Nothing starts an engine until a session asks for its model. `SIGTERM` and `SIGINT`
//! stop it gracefully: the bus name is released, every engine's process group is ended, and it
//! exits 0 (`inferd::shutdown`).
//!
//! The binary reads its arguments, starts listening for signals, asks `Config::from_env` for the
//! environment's answers, then builds and runs the daemon, all in the library
//! (`inferd::daemon`).

// The daemon's future holds the service's, whose domain traits each wrap their async bodies.
#![recursion_limit = "256"]

use clap::{Parser, Subcommand};
use inferd::shutdown::Signals;
use inferd::{Config, Daemon};
use std::path::PathBuf;
use std::process::ExitCode;

/// porter's AI broker (`org.quire.Inference1`).
#[derive(Debug, Parser)]
#[command(name = "inferd", version)]
struct Args {
    /// The configuration file, instead of `$XDG_CONFIG_HOME/inferd/inferd.toml`.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

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
    /// secret store inferd uses. The key is read with echo off and is never printed or logged.
    ///
    /// Each --allow gives that app the new account for language models: it records the
    /// permission an "Allow, always" answer to the app's consent sheet would. You typing this
    /// command is the consent. It covers every data class unless you narrow it with --class,
    /// and interactive use only; which classes may leave this computer is still decided by the
    /// ai.floor.<class> settings.
    ///
    /// inferd reads its keys only when it starts, so this command takes the bus name
    /// org.quire.Inference1 while it runs and stops if the daemon already owns it: stop the unit
    /// (systemctl --user stop inferd.service), run this, then start the unit again. Checking the
    /// key needs the network, so the unit's network drop-in must be installed.
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
    eprintln!("inferd: {why}");
    ExitCode::from(1)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    // Listening starts before anything else: a signal during start-up is kept, not fatal.
    let signals = match Signals::listen() {
        Ok(signals) => signals,
        Err(why) => return fail(format_args!("signal handlers: {why}")),
    };
    let config = match Config::from_env(args.config) {
        Ok(config) => config,
        Err(why) => return fail(why),
    };
    if let Some(Command::Add {
        provider,
        allow,
        classes,
    }) = &args.command
    {
        let done = inferd::keys::add::add_account(
            &config.dirs,
            config.keys.clone(),
            &config.bus,
            provider,
            allow,
            classes,
        )
        .await;
        return match done {
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
            Err(why) => fail(why),
        };
    }
    let daemon = match Daemon::build(config).await {
        Ok(daemon) => daemon,
        Err(why) => return fail(why),
    };
    match daemon.run(signals).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => fail(why),
    }
}
