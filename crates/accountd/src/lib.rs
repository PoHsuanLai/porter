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
//!   grant (`hub`); `Manager.Adopt` through the `[adopt]` table and a legacy store (`legacy`);
//!   `Peer` for the porter daemons; and `org.quire.SettingsModule1` at `settings_path()` for the
//!   Settings role, through quire's `ds_settings::live`.
//! - Roles: an `Agent` is refused `Choose`, `AddAccount`, `Reauthenticate`, `IssueToken`,
//!   `OpenAuthenticated` and `Adopt` (`Refusal::Denied`).
//! - `BusSheets` is the sheet link to the host over `org.quire.AccountsSheet1`.
//!
//! - `Tokens.OpenAuthenticated`: the grant and endpoint checked by the service, then a socketpair
//!   whose far end runs `porter_proxy::relay` (`relay`); the descriptor is returned once the relay
//!   has authenticated. `Options::relay_roots` is the test seam for the trusted roots.
//!
//! - `Peer.ResolveKey`: an API key of a granted Llm account on a sealed memfd, for a porter daemon
//!   only (`keys`); the key is read by `Options::keys`.
//!
//! - `Peer.ReportLocal`: a probed local runtime (Ollama, llama.cpp, LM Studio) as an account of its
//!   provider file, its models as claims, `offline` when it stops (`peer`); a porter daemon only.
//!

mod account;
pub mod add;
mod audit;
mod callers;
mod callers_file;
mod core;
mod errors;
mod grants;
mod hub;
mod keys;
mod legacy;
mod manager;
pub mod paths;
mod peer;
pub mod providers;
mod relay;
mod request;
mod roster;
mod settings;
mod settings_keys;
mod sheets;
mod store;
mod vardict;

pub use audit::FileAudit;
pub use callers::{Callers, TableCallers};
pub use callers_file::{CallerFileError, load_callers, table_from_file, table_from_toml};
pub use core::{Host, Options, serve, serve_with};
pub use errors::RefusedError;
pub use keys::{KeyDesk, RESOLVE_AUDIENCE, SecretsDesk, sealed_key};
pub use legacy::{AdoptConfig, AdoptTable, MemoryLegacy, Oo7Legacy, from_mailo};
pub use relay::RelayRoots;
pub use settings::settings_path;
pub use sheets::{BusLink, BusSheets};
pub use store::FileStore;
