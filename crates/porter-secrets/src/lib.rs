//! Where credentials live (design/31 §4.6): the [`Secrets`] trait accountd files every
//! password, refresh token and key through. Refresh tokens and passwords never leave the
//! daemon: nothing here is reachable from a wire type.
//!
//! Every store files a credential under a key, and the key is written the same way in each (the
//! Secret Service's attributes):
//!
//! ```
//! use porter_core::{AccountId, SecretKey, SecretPurpose};
//! use porter_secrets::{SERVICE, attributes};
//!
//! let key = SecretKey {
//!     account: AccountId::parse("cloud").expect("an id"),
//!     purpose: SecretPurpose::Password,
//! };
//! let filed = attributes(&key);
//! assert_eq!(
//!     (filed.service, filed.account.as_str(), filed.purpose.as_str()),
//!     (SERVICE, "cloud", r#"{"kind":"password"}"#)
//! );
//! ```

mod attributes;
mod error;
#[cfg(all(feature = "test-keys", unix))]
mod file;
#[cfg(feature = "keyring")]
mod keyring;
#[cfg(feature = "testing")]
mod memory;
#[cfg(feature = "oo7")]
mod oo7;
mod secrets;

pub use attributes::{SERVICE, SecretAttributes, attributes};
pub use error::SecretsError;
#[cfg(all(feature = "test-keys", unix))]
pub use file::{FileSecrets, FileSecretsError};
#[cfg(feature = "keyring")]
pub use keyring::{KeyringSecrets, StoreSecrets};
#[cfg(feature = "testing")]
pub use memory::MemorySecrets;
#[cfg(feature = "oo7")]
pub use oo7::{Oo7KeyringSecrets, Oo7Secrets};
pub use secrets::{PutOutcome, Secrets};
