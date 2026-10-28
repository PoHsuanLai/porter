//! The agent launcher's side of accountd: `org.quire.Accounts1.Launcher` at `/org/quire/Accounts1`.
//!
//! The agent launcher (docket-acp) registers the programs it launches, opens sessions, asks for
//! the consent sheet of an agent's key (`RequestAgentKey`, a Request object answered with a
//! permit), reports an agent's login or logout, and is handed an API key for a process it spawns
//! (`IssueProcessCredential`, a sealed memfd). Only a connection the caller table knows as the
//! agent launcher may call.
//!
//! The permits are grants of the hosted `porter_service::AccountService`. [`Launchers`] is the
//! roster of launchers, which the service's agent sign-in asks through
//! `porter_service_accounts::Launchers`.

mod credentials;
mod launchers;
mod object;
mod shared;
mod sheet;

pub(crate) use credentials::Credentials;
pub use launchers::{DEFAULT_BOUND, DEFAULT_TICK, Launchers, LoginTiming, SignOutNews};
pub(crate) use object::LauncherObject;
