//! Providers as data (design/31 §3): the TOML declaration format and its parser, the set of
//! installed providers, and the [`Provider`] trait each protocol family implements. Pure: the
//! caller reads files and runs futures.

mod error;
mod family;
mod parse;
mod provider;
mod set;
mod spec;

pub use error::{ProviderError, ProviderFileError};
pub use family::Family;
pub use parse::parse_provider;
pub use provider::{Presented, Provider, ProviderSession};
pub use set::ProviderSet;
pub use spec::{AiSpec, AuthSpec, CapabilityRow, Discovery, Endpoint, Issuer, Port, ProviderSpec};
