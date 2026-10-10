//! The app-facing API (design/31 §5.1). An app asks for a capability, never a brand:
//!
//! ```no_run
//! use porter_client::{Accounts, ClientError, Found, Transport};
//! use porter_core::consent::Usage;
//! use porter_core::wire::ParentWindow;
//! use porter_core::{Candidate, DataClass, Need};
//!
//! // `accounts` is `Accounts::connect(&env)` (D-Bus or the socket) or `Accounts::over(transport)`.
//! async fn photos_account<T: Transport>(
//!     accounts: &Accounts<T>,
//!     need: &Need,
//!     window: &ParentWindow,
//! ) -> Result<Option<Candidate>, ClientError> {
//!     let found = accounts.find(need, DataClass::Photos, Usage::Interactive).await?;
//!     Ok(match found {
//!         Found::One(candidate) => Some(candidate),
//!         Found::Several(list) => list.into_iter().next(), // quire's AccountPicker lets the person pick
//!         Found::NeedsConsent(offer) => Some(accounts.request_grant(&offer, window).await?),
//!         Found::None(_why) => None, // EmptyState + "Add Account…"
//!     })
//! }
//! ```
//!
//! Transport-agnostic behind [`Transport`]: D-Bus to accountd (feature `dbus`), the latchkey
//! socket where D-Bus is absent (feature `socket`), or [`InProcess`] where the app hosts the core
//! itself (feature `in-process`).
//!
//! Features (design/36 §1): the accounts core builds with `--no-default-features`, which reaches
//! no zbus, no porter-infer and no stoker. Inference (sessions, `prepare`, `infer`, `Need::Llm`
//! paths) is the default feature `infer`; an accounts-only consumer says
//! `default-features = false, features = ["socket"]`. `InProcess` and the account service it
//! hosts (porter-service, porter-secrets, porter-provider) are the default feature `in-process`;
//! a bus-only consumer says `default-features = false, features = ["dbus"]` and reaches none of
//! them.
//!
//! Settings' switch "Let my other computers use this computer's models" has a file to install
//! besides the setting: feature `lending` has `TailnetLending` (`enable`, `disable`, `state`),
//! which puts porter's shipped drop-in into the person's systemd user files and restarts inferd,
//! through a `UnitManager` the caller gives (`SessionUnits` with `dbus`). It never sets
//! `ai.tailnet.serve`; the caller does, after `enable` answers `Ok`. For detent's switch:
//!
//! ```ignore
//! // Features `lending` and `dbus`.
//! let config = LendingConfig::from_env().ok_or("no home folder")?;
//! let lending = TailnetLending::new(config, SessionUnits::session().await?);
//! match lending.state() {
//!     LendingState::Installed => { /* the switch shows "on" */ }
//!     LendingState::NotInstalled | LendingState::Differs => { /* the switch shows "off" */ }
//!     _ => {}
//! }
//! // The person turned the switch on and agreed:
//! match lending.enable().await {
//!     Ok(()) => { /* now set ai.tailnet.serve to "on" */ }
//!     Err(error) => show(error.to_string()), // plain words, ready to show
//! }
//! lending.disable().await?; // the switch turned off; the same again changes nothing more
//! ```
//!
//! A porter daemon (inferd, syncd) is also a client: [`peer::PeerAccounts`] (feature `dbus`) is
//! the typed way into accountd's daemon-only `Peer` surface (grant verdicts, the API key of a
//! granted account, a local runtime's report, the account news), and
//! [`Accounts::watch_removals`] hears that an account is gone.
//!
//! An app with no inferd turns on `engines` for [`engines::EngineHost`]: inference routed in the
//! app and sent to the OpenAI-compatible engines it points at (a local runtime, a company's API).

mod accounts;
mod authenticated;
#[cfg(feature = "infer")]
mod computers;
#[cfg(feature = "dbus")]
pub mod credential;
#[cfg(feature = "engines")]
pub mod engines;
mod env;
mod error;
mod found;
#[cfg(feature = "dbus")]
mod guests;
#[cfg(feature = "dbus")]
mod launcher;
#[cfg(feature = "dbus")]
pub mod peer;
mod relays;
#[cfg(feature = "dbus")]
mod removals;
#[cfg(feature = "dbus")]
mod spaces;
#[cfg(feature = "dbus")]
mod tailnet;
#[cfg(feature = "lending")]
mod tailnet_lending;
mod transport;

pub use accounts::Accounts;
pub use authenticated::{AuthenticatedStream, Relayed};
#[cfg(feature = "infer")]
pub use computers::{ComputerReach, NewComputer, NewComputerModel};
#[cfg(feature = "dbus")]
pub use credential::{
    ChildKey, ChildKeyError, CredentialHandle, Delivery, Inherit, ProcessCredential, Revocations,
    Revoked,
};
pub use env::{ClientEnv, LinkChoice, Place, START_WAIT, SocketAgent, StartAgent};
pub use error::{ClientError, ComputerReason, TransportError};
/// The refusal [`ClientError::InferRefused`] carries when there is no `infer` feature: a type with
/// no values (with the feature it is porter-infer's own).
#[cfg(not(feature = "infer"))]
pub use error::InferRefusal;
pub use found::{ConsentOffer, Found, NoAccount, found};
#[cfg(feature = "dbus")]
pub use guests::{GuestChange, GuestChanges};
#[cfg(feature = "dbus")]
pub use launcher::{AskKind, Launcher, LauncherError, LauncherRequest, Requests};
pub use porter_core::lending;
/// The computers that ask to use this one's models, and the ones that could be added, from
/// porter-core: what `Accounts::guests` and `Accounts::candidates` return and
/// `Accounts::answer_guest` takes, so Settings and the shell parse no vardict. The module
/// (`porter_client::lending`) also holds `Approval`, `RowState`, `GuestAsk` and
/// `CandidateModel`.
pub use porter_core::lending::{ComputerCandidate, GuestAnswer, GuestRow};
/// A computer on the person's Tailscale network, whose it is, and its stable id, from
/// porter-core: what `Tailnet::machines` returns, so a terminal or Settings parses no vardict.
pub use porter_core::{Machine, MachineOwner, NodeId};
/// The client's side of one `Open` fd, from porter-infer.
#[cfg(feature = "infer")]
pub use porter_infer::{
    ComputerName, InferSession, OpenOptions, PlaceId, SessionError, Traceparent,
};
/// What an [`engines::EngineHost`] is built from, from porter-infer.
#[cfg(feature = "engines")]
pub use porter_infer::{InferRefusal, Policy, Slot};
pub use relays::{NoRelays, RelayHost};
#[cfg(feature = "dbus")]
pub use removals::{Removals, RemovedAccount};
#[cfg(feature = "dbus")]
pub use spaces::{SpaceChanges, Spaces, SpacesError};
#[cfg(feature = "dbus")]
pub use tailnet::{MachineChanges, Tailnet, TailnetError};
#[cfg(all(feature = "lending", feature = "dbus"))]
pub use tailnet_lending::SessionUnits;
#[cfg(feature = "lending")]
pub use tailnet_lending::{
    INFERD_UNIT, LendingConfig, LendingError, LendingState, SHIPPED_NAME, TailnetLending,
    UnitFailure, UnitManager, UnitName,
};
#[cfg(feature = "in-process")]
pub use transport::InProcess;
#[cfg(feature = "infer")]
pub use transport::{AnySession, InProcessSession, SessionHost, SocketSession};
pub use transport::{AnyTransport, NoBroker, SocketTransport, Transport};
#[cfg(all(feature = "dbus", feature = "infer"))]
pub use transport::{DbusSession, MAX_ATTACHMENTS};
#[cfg(feature = "dbus")]
pub use transport::{DbusTransport, Rows};
