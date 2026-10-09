//! Providers as data (design/31 §3): the TOML declaration format and its parser, the set of
//! installed providers, and the [`Provider`] trait each protocol family implements. Pure: the
//! caller reads files and runs futures.
//!
//! The files porter ships are compiled in, so an app has the same providers with nothing
//! installed:
//!
//! ```
//! use porter_core::ProviderId;
//!
//! let providers = porter_provider::shipped();
//! let id = ProviderId::parse("nextcloud").expect("an id");
//! let nextcloud = providers.get(&id).expect("porter ships a Nextcloud file");
//! assert_eq!(nextcloud.id.as_str(), "nextcloud");
//! ```

mod clients;
mod error;
mod parse;
mod provider;
mod set;
mod shipped;
mod sign_in;
mod spec;

pub use clients::{
    ClientChannel, ClientEntry, ClientId, ClientsFile, ClientsFileError, parse_clients,
};
pub use error::{ProviderError, ProviderFileError};
pub use parse::parse_provider;
pub use provider::{Presented, Provider, ProviderSession, Readiness};
pub use set::ProviderSet;
pub use shipped::{SHIPPED_FILES, shipped, shipped_specs};
pub use sign_in::{RevokeOutcome, SignIn, SignInMode, SignInStart, SignInStep, Signed};
pub use spec::{
    AiSpec, AuthOrigin, AuthSpec, CapabilityRow, Discovery, DomainMatch, DomainName, Endpoint,
    Issuer, IssuerEndpoints, LinkedOrigin, Matching, Port, ProviderSpec,
};
