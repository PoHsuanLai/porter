//! accountd's bus front end as a library, so the objects it serves are tested on a private bus
//! without the daemon: `org.quire.Accounts1` over [`porter_service::AccountService`].
//!
//! - `Manager` (`Query`, `Availability`, `Choose`, `AddAccount`), `Grants` and `Tokens` at the
//!   accountd path; one `Account` object per account (`Reauthenticate`) at its own path.
//! - The caller is the connection's, never the request's: a [`Callers`] seam says which app a
//!   bus sender is, and a sender it does not know is refused `AccessDenied` by the bus's own
//!   error name (a client reports that as `TransportError::Denied`).
//! - A refusal is the error `org.quire.Accounts1.Error.<Refusal>` (`errors`), as the in-process
//!   carrier's `AccountsReply::Refused`.
//! - The three sheet methods return a Request object at once (`request`): the answer is its
//!   `Response` signal, sent to the caller alone, and the object goes when it has been sent. A
//!   `Close` from the caller, or the caller leaving the bus, ends the sheet and no `Response`
//!   follows.
//!
//! - `Account` properties (`Id`, `Provider`, `Label`, `State`, `Capabilities`), readable only with a
//!   grant for the account; the manager's five signals, unicast to the apps that hold a relevant
//!   grant (`hub`);
//!   `Peer` for the porter daemons; and `org.quire.SettingsModule1` at `settings_path()` for the
//!   Settings role, through quire's `ds_settings::live`.
//! - Roles: an `Agent` is refused `Choose`, `AddAccount`, `Reauthenticate`, `IssueToken`,
//!   `OpenAuthenticated` and `OpenLinked` (`Refusal::Denied`).
//! - `BusSheets` is the sheet link to the host over `org.quire.AccountsSheet1`.
//!
//! - `Tokens.OpenAuthenticated`: the grant and endpoint checked by the service, then a socketpair
//!   whose far end runs `porter_proxy::relay` (`relay`); the descriptor is returned once the relay
//!   has authenticated. `Options::relay_roots` is the test seam for the trusted roots.
//! - `Tokens.OpenLinked`: the same socketpair for an origin the account's provider file declares
//!   in `linked_origins` (a pre-authenticated link's host: Graph's `uploadUrl` and download
//!   redirect), checked by the service; the relay adds no credential and dials that origin only.
//!
//! - `Peer.ResolveKey`: an API key of a granted Llm account on a sealed memfd, for a porter daemon
//!   only (`keys`); the key is read by `Options::keys`.
//!
//! - `Peer.RegisterLauncher`, `Peer.ReportAgentLogin`, `Peer.ReportAgentLogout` and the unicast signals
//!   `AgentLoginRequested` and `AgentLogoutRequested` (`launchers`): the agent launcher registers the
//!   programs it runs, accountd asks it to sign an agent in or out, and it answers with a coarse
//!   outcome. The login itself never reaches accountd: no token, no URL, no code. `AgentLauncher`
//!   only.
//!
//! - `Tokens.IssueProcessCredential` and `Tokens.RevokeProcessCredential`, with the unicast signal
//!   `ProcessCredentialRevoked` (`handoff`, `credentials`): an API key for one process the agent
//!   launcher spawns, on a sealed memfd or in a 0600 tmpfs file, for an agent that cannot take a
//!   base URL. `AgentLauncher` only, for a program it registered; it ends with the launcher's
//!   connection, the grant or the account. Outside porter's meter (P4 is the metered route).
//!
//! - `Peer.ReportLocal`: a probed local runtime (Ollama, llama.cpp, LM Studio) as an account of its
//!   provider file, its models as claims, `offline` when it stops (`peer`); a porter daemon only.
//!
//! - Tailscale (`tailnet`, `tailnet_object`): the Tailscale account's state follows what
//!   Tailscale says (running and signed in `ok`, signed out `needs_login`, off or not running
//!   `offline`), read from its watch stream with a look every half minute behind it; and
//!   `org.quire.Tailnet1` at `/org/quire/Tailnet1` lists the person's other computers
//!   (`Machines`) with a `Changed` signal, for the shell, Settings and the terminal.
//!
//! - `org.quire.Spaces1` at `/org/quire/Spaces1`: the registry of desktop-wide Spaces, kept in
//!   `spaces.json` (`spaces`, `spaces_object`). Any identified app lists and creates (at most
//!   `CREATES_PER_WINDOW` a minute each); Settings and the shell rename, restyle and remove, and
//!   a removal ends the grants scoped to that Space.
//!

mod account;
pub mod add;
mod app_names;
mod audit;
mod callers;
mod client_rows;
mod core;
mod credentials;
mod errors;
mod grants;
mod handoff;
mod hub;
mod keys;
pub mod keysel;
mod launchers;
mod manager;
pub mod paths;
mod peer;
mod provider_names;
pub mod providers;
mod relay;
mod request;
mod roster;
mod settings;
mod settings_keys;
mod sheets;
mod spaces;
mod spaces_object;
mod store;
mod tailnet;
mod tailnet_object;

pub use app_names::AppNames;
pub use audit::FileAudit;
pub use callers::{Callers, TableCallers};
pub use core::{Host, Options, serve, serve_with};
pub use errors::RefusedError;
pub use keys::{KeyDesk, SecretsDesk, sealed_key};
pub use launchers::{DEFAULT_BOUND, DEFAULT_TICK, LoginTiming};
pub use porter_dbus::{
    CallerFileError, CallerTomlError, load_callers, table_from_file, table_from_toml,
};
pub use provider_names::ProviderNames;
pub use relay::RelayRoots;
pub use settings::settings_path;
pub use sheets::{BusLink, BusSheets};
pub use spaces::{CREATE_WINDOW, CREATES_PER_WINDOW, SpacesStore};
pub use store::{EXIT_REGISTRY_REFUSED, FileStore};
pub use tailnet::TailnetWatch;
