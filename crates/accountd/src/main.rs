//! accountd: the account registry, consent store and token broker on the session bus
//! (design/31 §4.1). A skeleton: it builds the service over its seams and exits, saying so.

mod clock;
mod families;
mod prompter;

use clap::Parser;
use clock::SystemClock;
use families::FamilyProvider;
use porter_secrets::Oo7Secrets;
use porter_service::{AccountService, Registry};
use prompter::SheetPrompter;
use std::path::PathBuf;
use std::process::ExitCode;

/// porter's account and consent service (`org.quire.Accounts1`).
#[derive(Debug, Parser)]
#[command(name = "accountd", version)]
struct Args {
    /// A directory of provider files, in addition to the system and user ones; later
    /// directories win.
    #[arg(long = "providers", value_name = "DIR")]
    provider_dirs: Vec<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    let service: AccountService<FamilyProvider, Oo7Secrets, SheetPrompter, SystemClock> =
        AccountService::new(
            Vec::new(),
            Registry::default(),
            Oo7Secrets,
            SheetPrompter,
            SystemClock,
        );
    let _ = (&service, &args.provider_dirs, porter_dbus::ACCOUNTS_BUS);
    // The daemon's one log path is standard error, prefixed with its name.
    eprintln!(
        "accountd: not implemented: {} is frozen as an interface (porter ARCHITECTURE.md §5); \
         the service does not serve yet",
        porter_dbus::ACCOUNTS_BUS
    );
    ExitCode::from(2)
}
