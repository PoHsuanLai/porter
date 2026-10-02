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
//! socket where D-Bus is absent, or [`InProcess`] where the app hosts the core itself.

mod accounts;
mod env;
mod error;
mod found;
mod transport;

pub use accounts::Accounts;
pub use env::{ClientEnv, LinkChoice, SocketPath};
pub use error::{ClientError, TransportError};
pub use found::{ConsentOffer, Found, NoAccount, found};
/// The client's side of one `Open` fd, from porter-infer.
pub use porter_infer::{InferSession, OpenOptions, SessionError, Traceparent};
pub use transport::{
    AnySession, AnyTransport, InProcess, InProcessSession, SocketSession, SocketTransport,
    Transport,
};
#[cfg(feature = "dbus")]
pub use transport::{DbusSession, DbusTransport};
