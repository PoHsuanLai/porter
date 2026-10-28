//! accountd's bus front end as a library, so the objects it serves are tested on a private bus
//! without the daemon: `org.quire.Accounts1` over [`porter_service::AccountService`].
//!
//! accountd holds identities (the accounts a person signs in to: OAuth, a password, an app
//! password), Spaces and Tailscale, and the AI keys with the grants that let an app use them:
//! one registry, one secret namespace, one grant store. The agent logins and the local runtimes
//! the AI broker reports are accounts of it too. syncd signs in for itself.
//!
//! - `Manager` (`AddAccount`, `NeedingReauth`, the signals `AccountAdded`, `AccountRemoved` and
//!   `NeedsReauth`) at the accountd path; one `Account` object per account (`Reauthenticate`) at
//!   its own path.
//! - The caller is the connection's, never the request's: a [`Callers`] seam says which app a
//!   bus sender is, and a sender it does not know is refused `AccessDenied` by the bus's own
//!   error name (a client reports that as `TransportError::Denied`).
//! - A refusal is the error `org.quire.Accounts1.Error.<Refusal>` (`errors`), as the in-process
//!   carrier's `AccountsReply::Refused`.
//! - The two sheet methods return a Request object at once (`request`, over porter-sheets'
//!   `RequestObjects`): the answer is its `Response` signal, sent to the caller alone, and the
//!   object goes when it has been sent. A `Close` from the caller, or the caller leaving the
//!   bus, ends the sheet and no `Response` follows.
//!
//! - `Account` properties (`Id`, `Provider`, `Label`, `State`, `ReauthReason`, `Capabilities`),
//!   readable only by the shell (`SheetHost`) and Settings; the manager's signals, unicast to
//!   those two (`hub`); and `org.quire.SettingsModule1` at `settings_path()` for the Settings
//!   role, through quire's `ds_settings::live`: one block per account, the apps using each
//!   identity (`accounts.<id>.used_by.<n>`, read-only), a switch for each AI service and a Revoke
//!   for each permit, and the client ids.
//! - Roles: an `Agent` is refused `AddAccount`, `Reauthenticate` and `RequestKey`
//!   (`Refusal::Denied`); the agent launcher may call `Launcher` and nothing else; the AI broker
//!   may call `Broker` and read the computers on the tailnet, and nothing else.
//! - `BusSheets` is the sheet link to the host over `org.quire.AccountsSheet1`
//!   (porter-sheets).
//!
//! - The AI keys: `org.quire.Accounts1.Keys` (`keys`: an app's `RequestKey`, answered with the
//!   permit it already holds or a consent sheet), `org.quire.Accounts1.Launcher` (`launcher`:
//!   the agent launcher's programs, sessions, the sheet for an agent's key, the logins and the key
//!   handed to a process it spawns on a sealed memfd) and `org.quire.Accounts1.Broker`
//!   (`broker`: what an app's permits say and the key behind one, for inferd alone). The keys
//!   inferd held before are copied in at the start (`import`).
//!
//! - Tailscale (`tailnet`, `tailnet_object`): the Tailscale account's state follows what
//!   Tailscale says (running and signed in `ok`, signed out `needs_login`, off or not running
//!   `offline`), read from its watch stream with a look every half minute behind it; and
//!   `org.quire.Tailnet1` at `/org/quire/Tailnet1` lists the person's other computers
//!   (`Machines`) with a `Changed` signal, for the shell, Settings, the terminal and the AI
//!   broker.
//!
//! - `org.quire.Spaces1` at `/org/quire/Spaces1`: the registry of desktop-wide Spaces, kept in
//!   `spaces.json` (`spaces`, `spaces_object`). Any identified app lists and creates (at most
//!   `porter_spaces::CREATES_PER_WINDOW` a minute each); Settings and the shell rename, restyle and remove.
//!
//! - `org.quire.Identities1` at `/org/quire/Identities1`: the identities signed in on this
//!   machine, which are the registry's accounts of a sign-in kind (OAuth, a password, an app
//!   password; `identities_object`). Any app lists them, `Offer`s "Continue as" (the picker is the
//!   sheet `Choose` draws, `AskPurpose::ContinueAs`, through a `porter_identities::IdentityDesk`; the answer is the
//!   pick, never a token) and says it signed in with one (`RegisterUse`, kept in
//!   `identity-uses.json` beside `registry.json`: `identities`); Settings and the shell read
//!   `Uses`. `TakePassword` hands a `password` identity's password on a sealed descriptor, once, to
//!   the app the person just picked it for, and is audited. `IdentityRemoved` goes out when
//!   the registry loses an identity. The agent, computer use, the agent launcher, the AI broker, the terminal and
//!   the compositor are refused.
//!
//! - The daemon as one call (`daemon`): [`Config`] (typed; [`Config::from_env`] is the one place
//!   the environment is read), [`Daemon::build`] and [`Daemon::run`]. The binary only parses its
//!   arguments and calls these.
//!

mod account;
pub mod add;
mod audit;
mod broker;
mod callers;
mod core;
pub mod daemon;
mod errors;
mod host;
mod hub;
mod identities_object;
pub mod import;
mod keys;
pub mod launcher;
mod manager;
mod news;
mod open_sheets;
pub mod paths;
mod publish;
mod request;
mod roster;
mod settings;
mod spaces_object;
mod state;
mod store;
mod tailnet;
mod tailnet_follow;
mod tailnet_object;

pub use audit::FileAudit;
pub use callers::{Callers, TableCallers};
pub use core::{Options, serve, serve_with};
pub use daemon::{Config, Daemon, StartError};
pub use errors::RefusedError;
pub use host::Host;
pub use news::NewsStore;
#[doc(inline)]
pub use porter_families::set::ProviderNames;
#[doc(inline)]
pub use porter_sheets::{AppNames, BusLink, BusSheets};
pub use settings::settings_path;
pub use store::EXIT_REGISTRY_REFUSED;
pub use tailnet::TailnetWatch;
