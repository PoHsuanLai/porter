//! accountd: the identities signed in on this computer, Spaces and Tailscale, on the session bus
//! (design/31 §4.1).
//!
//! The binary reads its arguments, asks `Config::from_env` for the environment's answers, then
//! builds and runs the daemon, all in the library (`accountd::daemon`): it resolves its paths,
//! loads the caller tables, the provider files and the registry (refusing to start over a
//! registry it cannot read), builds the service over its seams (the Secret Service through oo7 in
//! the `porter.accountd` namespace, or in a `test-keys` build the key file `ACCOUNTD_KEYS` names,
//! the sheet host over `org.quire.AccountsSheet1`, the families, the file store and audit) and
//! serves `org.quire.Accounts1`. Adding an AI key is `inferd add`.

use accountd::{Config, Daemon, StartError};
use clap::Parser;
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
    let daemon = match Daemon::build(config).await {
        Ok(daemon) => daemon,
        Err(why) => return fail(&why, StartError::exit_code(&why)),
    };
    // Serves until the session ends or the unit stops it (SIGTERM ends the process).
    daemon.run(std::future::pending()).await;
    ExitCode::SUCCESS
}
