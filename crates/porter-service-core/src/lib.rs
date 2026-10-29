//! What the service domains share (design/31 §4.1): the registry and the pure answers over it,
//! the audit trail, the sheets seam, the reach policy, the per-account turns at an issuer, and
//! the [`Ctx`] (the seams and the registry) every domain works on. The domains are crates of
//! their own over it: `porter-service-accounts` and `porter-service-grants`; `porter-service`
//! is the facade a host holds.
//!
//! A seam's failure is a small value with a plain sentence:
//!
//! ```
//! use porter_sheet::host::SheetFault;
//!
//! assert_eq!(SheetFault::Closed.to_string(), "sheet closed");
//! ```
//!
//! This crate depends on no executor, so the file store lives in `porter-store`.

pub mod audit;
pub mod consent;
mod credential;
mod ctx;
pub mod flight;
pub mod gate;
pub mod reach;
pub mod refusal;
pub mod registry;

pub use ctx::{Ctx, Parts, Ports};
