//! accountd's core (design/31 §4.1): the account registry, consent and the token broker over
//! its seams — [`porter_provider::Provider`] (protocol code), [`porter_secrets::Secrets`],
//! [`Sheets`] (the host that draws consent and sign-in), a [`Clock`], a [`RegistryStore`]
//! (where the registry lives between runs) and an [`AuditSink`]. Transport-free: accountd hosts
//! it behind D-Bus and the socket, and an app may host it in process.

mod add;
mod add_flow;
mod add_store;
mod adopt;
mod audience;
mod audit;
mod choose;
mod clock;
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
pub use adopt::{LegacyFault, LegacyStore, legacy_entry};
pub use audit::{AuditSink, NoAudit};
pub use clock::Clock;
pub use local::{LocalFault, MAX_LOCAL_CLAIMS};
pub use manage::RevokeReport;
pub use registry::Registry;
pub use service::AccountService;
pub use sheets::{SheetFault, SheetLink, SheetOpen, Sheets};
pub use store::{NoStore, RegistryStore, StoreError};
pub use sync_grant::{SyncClass, sync_allowed, sync_app, sync_key, sync_offers};
pub use token::secret_purpose;
