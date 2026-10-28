//! The app-facing API (design/31 §5.1). An app asks for a capability, never a brand. The
//! account calls below (`find`, `request_grant`, relays) are answered by a service the
//! app hosts in process ([`InProcess`]); over the buses accountd and inferd keep what they hold
//! and the app asks them for an identity or a key permit:
//!
//! ```no_run
//! # #[cfg(feature = "in-process")]
//! # mod example {
//! use porter_client::{Accounts, ClientError, Found, HostedCore, InProcess};
//! use porter_core::consent::Usage;
//! use porter_wire::ParentWindow;
//! use porter_core::{Candidate, DataClass};
//! use porter_match::Need;
//!
//! // `accounts` is `Accounts::over(InProcess::new(service, app))`: `find` is offered only
//! // where the service is hosted.
//! async fn photos_account<C: HostedCore>(
//!     accounts: &Accounts<InProcess<C>>,
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
//! # }
//! # fn main() {}
//! ```
//!
//! Transport-agnostic behind [`Transport`]: D-Bus to accountd (feature `dbus`), or [`InProcess`]
//! where the app hosts the core itself (feature `in-process`).
//!
//! Features (design/36 §1): no feature is on by default, and the accounts core with none reaches
//! no zbus, no porter-infer and no stoker. Inference (sessions, `prepare`, `infer`, `Need::Llm`
//! paths) is the feature `infer`. `InProcess` and the account service it hosts (porter-service,
//! porter-secrets, porter-provider) are the feature `in-process`; a bus-only consumer says
//! `features = ["dbus"]` and reaches none of them.
//!
//! Settings' switch "Let my other computers use this computer's models" has a file to install
//! besides the setting: feature `lending` has `TailnetLending` (`enable`, `disable`, `state`),
//! which puts porter's shipped drop-in into the person's systemd user files and restarts inferd,
//! through a `UnitManager` the caller gives (`SessionUnits` with `dbus`). It never sets
//! `ai.tailnet.serve`; the caller does, after `enable` answers `Ok`. For detent's switch:
//!
//! ```ignore
//! // Features `lending` and `dbus`.
//! let config = LendingConfig::from_lookup(|name| std::env::var_os(name))
//!     .ok_or("no home folder")?;
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
//! Whoever uses a credential holds it, so no credential crosses a bus. Over D-Bus an app asks
//! accountd's `Identities1` for "Continue as" ([`Identities`]) and inferd's `Keys` for a permit to
//! use the person's AI keys ([`AiKeys`]); an app or a daemon that needs tokens and relays hosts
//! its own `AccountService` and reaches it through [`InProcess`] (with [`PorterRelays`], feature
//! `relays`, running the relays). [`Accounts::watch_removals`] hears that an identity is gone.
//!
//! An app with no inferd turns on `engines` for [`engines::EngineHost`]: inference routed in the
//! app and sent to the OpenAI-compatible engines it points at (a local runtime, a company's API).

#[cfg(feature = "dbus")]
mod abandon;
mod accounts;
#[cfg(feature = "dbus")]
mod ai_accounts;
#[cfg(feature = "dbus")]
mod ai_keys;
mod authenticated;
#[cfg(feature = "dbus")]
mod call_failure;
#[cfg(feature = "infer")]
mod computers;
#[cfg(feature = "dbus")]
pub mod credential;
#[cfg(feature = "dbus")]
mod destination;
#[cfg(feature = "engines")]
pub mod engines;
mod error;
#[cfg(feature = "in-process")]
mod find;
mod found;
#[cfg(feature = "dbus")]
mod guests;
#[cfg(feature = "dbus")]
mod identities;
#[cfg(feature = "dbus")]
mod launcher;
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
#[cfg(feature = "dbus")]
pub use ai_accounts::{AiAccounts, AiAccountsError, Changes};
#[cfg(feature = "dbus")]
pub use ai_keys::{AiKeys, AiKeysError};
pub use authenticated::{AuthenticatedStream, Relayed};
#[cfg(feature = "dbus")]
pub use call_failure::CallFailure;
#[cfg(feature = "infer")]
pub use computers::{ComputerReach, NewComputer, NewComputerModel};
#[cfg(feature = "dbus")]
pub use credential::{
    ChildKey, ChildKeyError, Delivery, Inherit, ProcessCredential, Revocations, Revoked,
};
#[cfg(feature = "dbus")]
pub use destination::Destination;
/// The refusal [`ClientError::InferRefused`] carries when there is no `infer` feature: a type with
/// no values (with the feature it is porter-infer's own).
#[cfg(not(feature = "infer"))]
pub use error::InferRefusal;
pub use error::{ClientError, ComputerReason, TransportError};
pub use found::{ConsentOffer, Found, NoAccount, found};
#[cfg(feature = "dbus")]
pub use guests::{GuestChange, GuestChanges};
#[cfg(feature = "dbus")]
pub use identities::{Identities, IdentitiesError, IdentityRemovals, Picked};
#[cfg(feature = "dbus")]
pub use launcher::{AskKind, KeyCandidate, Launcher, LauncherError, LauncherRequest, Requests};
/// The client's side of one `Open` fd, from porter-infer.
#[cfg(feature = "infer")]
pub use porter_infer::{
    ComputerName, InferSession, OpenOptions, PlaceId, SessionError, Traceparent,
};
/// What an [`engines::EngineHost`] is built from, from porter-infer.
#[cfg(feature = "engines")]
pub use porter_infer::{InferRefusal, Policy, Slot};
#[cfg(feature = "relays")]
pub use relays::PorterRelays;
pub use relays::{NoRelays, RelayHost};
#[cfg(feature = "dbus")]
pub use removals::{Removals, RemovedAccount};
#[cfg(feature = "dbus")]
pub use spaces::{SpaceChanges, Spaces, SpacesError};
#[cfg(feature = "dbus")]
pub use tailnet::{MachineChanges, Tailnet};
#[cfg(all(feature = "lending", feature = "dbus"))]
pub use tailnet_lending::SessionUnits;
#[cfg(feature = "lending")]
pub use tailnet_lending::{
    INFERD_UNIT, LendingConfig, LendingError, LendingState, SHIPPED_NAME, TailnetLending,
    UnitFailure, UnitManager,
};
#[cfg(all(feature = "dbus", feature = "infer"))]
pub use transport::{DbusSession, MAX_ATTACHMENTS};
#[cfg(feature = "dbus")]
pub use transport::{DbusTransport, Rows};
#[cfg(feature = "in-process")]
pub use transport::{HostedCore, InProcess};
#[cfg(feature = "infer")]
pub use transport::{InProcessSession, SessionHost};
pub use transport::{NoBroker, Transport};
