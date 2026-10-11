//! Where credentials live (design/31 §4.6): the [`Secrets`] trait accountd files every
//! password, refresh token and key through. Refresh tokens and passwords never leave the
//! daemon: nothing here is reachable from a wire type.
//!
//! Every store files a credential under a key, and the key is written the same way in each (the
//! Secret Service's attributes):
//!
//! ```
//! use porter_core::{AccountId, SecretKey, SecretPurpose};
//! use porter_secrets::{Namespace, SERVICE, attributes, attributes_in};
//!
//! let key = SecretKey {
//!     account: AccountId::parse("cloud").expect("an id"),
//!     purpose: SecretPurpose::Password,
//! };
//! let filed = attributes(&key);
//! assert_eq!(
//!     (filed.service.as_str(), filed.account.as_str(), filed.purpose.as_str()),
//!     (SERVICE, "cloud", r#"{"kind":"password"}"#)
//! );
//!
//! // A host that keeps its own credentials names its own namespace; porter's daemons keep
//! // `Namespace::PORTER`, which is what every constructor without a namespace uses.
//! let mailo = Namespace::parse("mailo/work").expect("a namespace");
//! assert_eq!(attributes_in(&mailo, &key).service, "mailo/work");
//! assert_eq!(attributes_in(&Namespace::PORTER, &key), filed);
//! ```
//!
//! The namespace is the `service` attribute (and the keyring-core service). `Oo7Secrets`,
//! `KeyringSecrets` and `FileSecrets` file under `Namespace::PORTER`; `Oo7Secrets::in_namespace`,
//! `KeyringSecrets::in_namespace` and `FileSecrets::open_in` take another.

mod attributes;
mod error;
#[cfg(all(feature = "test-keys", unix))]
mod file;
#[cfg(feature = "keyring")]
mod keyring;
#[cfg(feature = "oo7")]
pub mod keysel;
#[cfg(feature = "testing")]
mod memory;
mod namespace;
#[cfg(feature = "oo7")]
mod oo7;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod sealed;
mod secrets;

pub use attributes::{SERVICE, SecretAttributes, attributes, attributes_in};
pub use error::SecretsError;
#[cfg(all(feature = "test-keys", unix))]
pub use file::{FileSecrets, FileSecretsError};
#[cfg(feature = "keyring")]
pub use keyring::{KeyringNamedSecrets, KeyringSecrets, StoreSecrets};
#[cfg(feature = "testing")]
pub use memory::MemorySecrets;
pub use namespace::{Namespace, NamespaceError};
#[cfg(feature = "oo7")]
pub use oo7::{Oo7KeyringSecrets, Oo7NamedSecrets, Oo7Secrets};
#[cfg(any(target_os = "linux", target_os = "android"))]
pub use sealed::sealed_key;
pub use secrets::{PutOutcome, Secrets};
