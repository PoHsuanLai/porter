//! accountd's core (design/31 §4.1): the account registry, consent and the token broker over
//! its seams — [`porter_provider::Provider`] (protocol code), [`porter_secrets::Secrets`],
//! [`Sheets`] (the host that draws consent and sign-in), a [`Clock`], a [`RegistryStore`]
//! (where the registry lives between runs) and an [`AuditSink`]. Transport-free: accountd hosts
//! it behind D-Bus and the socket, and an app may host it in process.

mod add;
mod audience;
mod audit;
mod choose;
mod clock;
mod registry;
mod service;
mod sheets;
mod store;
mod token;

pub use audit::{AuditSink, NoAudit};
pub use clock::Clock;
pub use registry::Registry;
pub use service::AccountService;
pub use sheets::{SheetFault, SheetLink, SheetOpen, Sheets};
pub use store::{NoStore, RegistryStore, StoreError};
pub use token::secret_purpose;
