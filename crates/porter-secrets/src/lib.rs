//! Where credentials live (design/31 §4.6): the [`Secrets`] trait accountd files every
//! password, refresh token and key through. Refresh tokens and passwords never leave the
//! daemon: nothing here is reachable from a wire type.

mod attributes;
mod error;
#[cfg(feature = "keyring")]
mod keyring;
#[cfg(feature = "testing")]
mod memory;
#[cfg(feature = "oo7")]
mod oo7;
mod secrets;

pub use attributes::{SERVICE, SecretAttributes, attributes};
pub use error::SecretsError;
#[cfg(feature = "keyring")]
pub use keyring::KeyringSecrets;
#[cfg(feature = "testing")]
pub use memory::MemorySecrets;
#[cfg(feature = "oo7")]
pub use oo7::Oo7Secrets;
pub use secrets::Secrets;
