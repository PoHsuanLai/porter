//! accountd's core (design/31 §4.1): the account registry, consent and the token broker over
//! its seams — [`porter_provider::Provider`] (protocol code), [`porter_secrets::Secrets`],
//! [`Sheets`] (the host that draws consent and sign-in), a [`Clock`], a [`RegistryStore`]
//! (where the registry lives between runs) and an [`AuditSink`]. Transport-free: accountd hosts
//! it behind D-Bus and the socket, and an app may host it in process.
//!
//! The seams' failures are small values with plain sentences, and what syncd may keep of an
//! account is read from the grants (none held, none allowed):
//!
//! ```
//! use porter_core::AccountId;
//! use porter_service::{SheetFault, StoreError, SyncClass, sync_allowed};
//!
//! let account = AccountId::parse("cloud").expect("an id");
//! for class in SyncClass::ALL {
//!     assert!(!sync_allowed(&[], &account, class));
//! }
//! assert_eq!(SheetFault::Closed.to_string(), "sheet closed");
//! assert_eq!(StoreError::Unavailable.to_string(), "registry store unavailable");
//! ```

mod add;
mod add_flow;
mod add_store;
mod agent;
mod agent_login;
mod audience;
mod audit;
mod choose;
mod clock;
mod gate;
mod local;
mod manage;
mod race;
mod registry;
mod relay;
mod service;
mod sheets;
mod store;
mod sync_grant;
mod token;

pub use add_flow::AllowFor;
pub use agent::AgentFault;
pub use agent_login::{Launchers, LoginEnd, NoLauncher, Roster, Waiting, program_of};
pub use audit::{AuditSink, NoAudit};
pub use clock::Clock;
pub use local::{LocalFault, MAX_LOCAL_CLAIMS};
pub use manage::RevokeReport;
pub use registry::Registry;
pub use service::AccountService;
pub use sheets::{SheetFault, SheetLink, SheetOpen, Sheets};
pub use store::{NoStore, RegistryStore, StoreError};
pub use sync_grant::{
    SyncClass, photos_key, sync_allowed, sync_app, sync_key, sync_key_of, sync_offers,
};
pub use token::secret_purpose;
