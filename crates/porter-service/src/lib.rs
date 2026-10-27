//! accountd's core (design/31 §4.1): the account registry, consent and the token broker over
//! its seams — [`porter_provider::Provider`] (protocol code), [`porter_secrets::Secrets`],
//! [`Sheets`] (the host that draws consent and sign-in), a [`Clock`], a [`RegistryStore`]
//! (where the registry lives between runs) and an [`AuditSink`]. Transport-free: accountd hosts
//! it behind D-Bus and the socket, and an app may host it in process.
//!
//! An OAuth account is refreshed one caller at a time inside a service: a second caller waits and
//! then uses the token the first one issued (one call to the issuer serves both), and a refresh
//! token the issuer rotated is stored before the new access token is handed out. A host that
//! keeps its registry on disk passes `porter_store::FileStore` to
//! [`AccountService::with_store`]; this crate depends on no executor, so the file store lives in
//! `porter-store`.
//!
//! The seams' failures are small values with plain sentences:
//!
//! ```
//! use porter_service::{SheetFault, StoreError};
//!
//! assert_eq!(SheetFault::Closed.to_string(), "sheet closed");
//! assert_eq!(StoreError::Unavailable.to_string(), "registry store unavailable");
//! ```

mod add;
mod add_flow;
mod add_store;
mod agent;
mod agent_login;
mod audience;
pub mod audit;
mod choose;
mod flight;
mod gate;
mod local;
mod manage;
mod own_machines;
mod race;
mod registry;
mod relay;
mod service;
mod sheets;
mod store;
mod token;

pub use add_flow::{AllowFor, SignInAs};
pub use agent::AgentFault;
pub use agent_login::{Launchers, LoginEnd, NoLauncher, Roster, Waiting, program_of};
pub use audit::{AuditSink, NoAudit};
pub use local::{LocalFault, MAX_LOCAL_CLAIMS};
pub use manage::RevokeReport;
pub use own_machines::OwnMachines;
pub use registry::Registry;
pub use service::AccountService;
pub use sheets::{SheetFault, SheetLink, SheetOpen, Sheets};
pub use store::{NoStore, RegistryStore, StoreError};
pub use token::secret_purpose;
