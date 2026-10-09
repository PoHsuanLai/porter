//! The app-facing API (design/31 §5.1). An app asks for a capability, never a brand:
//!
//! ```ignore
//! let accounts = Accounts::connect(&env).await?;
//! let need = Need::Storage(StorageNeed { access: ReadWrite, delta: Poll, scope: AppFolder, quota: Unreported });
//! match accounts.find(&need, DataClass::Photos, Usage::Interactive).await? {
//!     Found::One(candidate) => use_it(candidate),
//!     Found::Several(list) => pick_among(list),              // quire's AccountPicker
//!     Found::NeedsConsent(offer) => accounts.request_grant(&offer, &window).await?,
//!     Found::None(why) => show_no_account(why),               // EmptyState + "Add Account…"
//! }
//! ```
//!
//! Transport-agnostic behind [`Transport`]: D-Bus to accountd (feature `dbus`), the latchkey
//! socket where D-Bus is absent (feature `socket`), or [`InProcess`] where the app hosts the core
//! itself.
//!
//! Features (design/36 §1): the accounts core builds with `--no-default-features`, which reaches
//! no zbus, no porter-infer and no stoker. Inference (sessions, `prepare`, `infer`, `Need::Llm`
//! paths) is the default feature `infer`; an accounts-only consumer says
//! `default-features = false, features = ["socket"]`.
//!
//! An app with no inferd turns on `engines` for [`engines::EngineHost`]: inference routed in the
//! app and sent to the OpenAI-compatible engines it points at (a local runtime, a company's API).

mod accounts;
mod authenticated;
#[cfg(feature = "dbus")]
pub mod credential;
#[cfg(feature = "engines")]
pub mod engines;
mod env;
mod error;
mod found;
#[cfg(feature = "dbus")]
mod launcher;
mod relays;
#[cfg(feature = "dbus")]
mod spaces;
#[cfg(feature = "dbus")]
mod tailnet;
mod transport;

pub use accounts::Accounts;
pub use authenticated::{AuthenticatedStream, Relayed};
#[cfg(feature = "dbus")]
pub use credential::{
    ChildKey, ChildKeyError, CredentialHandle, Delivery, Inherit, ProcessCredential, Revocations,
    Revoked,
};
pub use env::{ClientEnv, LinkChoice, Place, START_WAIT, SocketAgent, StartAgent};
pub use error::{ClientError, TransportError};
pub use found::{ConsentOffer, Found, NoAccount, found};
#[cfg(feature = "dbus")]
pub use launcher::{AskKind, Launcher, LauncherError, LauncherRequest, Requests};
/// A computer on the person's Tailscale network, whose it is, and its stable id, from
/// porter-core: what `Tailnet::machines` returns, so a terminal or Settings parses no vardict.
pub use porter_core::{Machine, MachineOwner, NodeId};
/// What an [`engines::EngineHost`] is built from, from porter-infer.
#[cfg(feature = "engines")]
pub use porter_infer::{InferRefusal, Policy, Slot};
/// The client's side of one `Open` fd, from porter-infer.
#[cfg(feature = "infer")]
pub use porter_infer::{InferSession, OpenOptions, SessionError, Traceparent};
pub use relays::{NoRelays, RelayHost};
#[cfg(feature = "dbus")]
pub use spaces::{SpaceChanges, Spaces, SpacesError};
#[cfg(feature = "dbus")]
pub use tailnet::{MachineChanges, Tailnet, TailnetError};
#[cfg(feature = "dbus")]
pub use transport::DbusTransport;
#[cfg(feature = "infer")]
pub use transport::{AnySession, InProcessSession, SessionHost, SocketSession};
pub use transport::{AnyTransport, InProcess, NoBroker, SocketTransport, Transport};
#[cfg(all(feature = "dbus", feature = "infer"))]
pub use transport::{DbusSession, MAX_ATTACHMENTS};
