//! The app-facing API of the account service (design/31 §5.1). An app asks for a capability, never
//! a brand. The account calls below (`find`, `request_grant`, relays) are answered by a service
//! the app hosts in process ([`InProcess`]); over the bus accountd keeps what it holds and the app
//! asks it for an identity or a key permit:
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
//! no zbus. `InProcess` and the account service it hosts (porter-service, porter-secrets,
//! porter-provider) are the feature `in-process`; a bus-only consumer says `features = ["dbus"]`
//! and reaches none of them. Inference (sessions, `prepare`, computers, guests, engines, the
//! tailnet drop-in) is `inferd-client`'s.
//!
//! Whoever uses a credential holds it, so no credential crosses a bus. Over D-Bus an app asks
//! accountd's `Identities1` for "Continue as" ([`Identities`]) and for a permit to use the person's
//! AI keys ([`AiKeys`]); an app or a daemon that needs tokens and relays hosts its own
//! `AccountService` and reaches it through [`InProcess`] (with [`PorterRelays`], feature `relays`,
//! running the relays). [`Accounts::watch_removals`] hears that an identity is gone.

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
#[cfg(feature = "dbus")]
pub mod credential;
#[cfg(feature = "dbus")]
mod destination;
mod error;
#[cfg(feature = "in-process")]
mod find;
mod found;
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
mod transport;

pub use accounts::Accounts;
#[cfg(feature = "dbus")]
pub use ai_accounts::{AiAccounts, AiAccountsError, Changes};
#[cfg(feature = "dbus")]
pub use ai_keys::{AiKeys, AiKeysError};
pub use authenticated::{AuthenticatedStream, Relayed};
#[cfg(feature = "dbus")]
pub use call_failure::CallFailure;
#[cfg(feature = "dbus")]
pub use credential::{
    ChildKey, ChildKeyError, Delivery, Inherit, ProcessCredential, Revocations, Revoked,
};
#[cfg(feature = "dbus")]
pub use destination::Destination;
pub use error::{ClientError, TransportError};
pub use found::{ConsentOffer, Found, NoAccount, found};
#[cfg(feature = "dbus")]
pub use identities::{Identities, IdentitiesError, IdentityRemovals, Picked};
#[cfg(feature = "dbus")]
pub use launcher::{AskKind, KeyCandidate, Launcher, LauncherError, LauncherRequest, Requests};
#[cfg(feature = "relays")]
pub use relays::PorterRelays;
pub use relays::{NoRelays, RelayHost};
#[cfg(feature = "dbus")]
pub use removals::{Removals, RemovedAccount};
#[cfg(feature = "dbus")]
pub use spaces::{SpaceChanges, Spaces, SpacesError};
#[cfg(feature = "dbus")]
pub use tailnet::{MachineChanges, Tailnet};
#[cfg(feature = "dbus")]
pub use transport::DbusTransport;
pub use transport::Transport;
#[cfg(feature = "in-process")]
pub use transport::{HostedCore, InProcess};
