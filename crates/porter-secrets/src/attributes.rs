//! How a [`SecretKey`] is written as Secret Service attributes: `{service, account, purpose}`.

use crate::namespace::Namespace;
use porter_core::SecretKey;

/// The `service` attribute of every item porter's own daemons file: [`Namespace::PORTER`].
pub const SERVICE: &str = "porter";

/// The attributes of one item, in the order the store is searched by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretAttributes {
    /// The namespace's name: [`SERVICE`] unless the host chose another.
    pub service: String,
    /// The account id.
    pub account: String,
    /// The purpose in its serde form, which is its stored schema.
    pub purpose: String,
}

/// The attributes `key` is filed under in porter's own namespace.
pub fn attributes(key: &SecretKey) -> SecretAttributes {
    attributes_in(&Namespace::PORTER, key)
}

/// The attributes `key` is filed under in `namespace`.
pub fn attributes_in(namespace: &Namespace, key: &SecretKey) -> SecretAttributes {
    SecretAttributes {
        service: namespace.as_str().to_owned(),
        account: key.account.to_string(),
        // A fieldless or `CapabilityKind`-carrying enum always serializes.
        purpose: serde_json::to_string(&key.purpose).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::{AccountId, CapabilityKind, SecretPurpose};

    #[test]
    fn purposes_are_filed_by_their_serde_form() {
        let cases = [
            (SecretPurpose::Password, r#"{"kind":"password"}"#),
            (SecretPurpose::OAuthRefresh, r#"{"kind":"oauth_refresh"}"#),
            (
                SecretPurpose::ServicePassword(CapabilityKind::Contacts),
                r#"{"kind":"service_password","v":"contacts"}"#,
            ),
        ];
        let mailo = Namespace::parse("mailo/work").expect("namespace");
        for (purpose, want) in cases {
            let key = SecretKey {
                account: AccountId::parse("cloud").expect("id"),
                purpose,
            };
            let got = attributes(&key);
            assert_eq!(
                (
                    got.service.as_str(),
                    got.account.as_str(),
                    got.purpose.as_str()
                ),
                ("porter", "cloud", want)
            );
            let named = attributes_in(&mailo, &key);
            assert_eq!(
                (
                    named.service.as_str(),
                    named.account.as_str(),
                    named.purpose.as_str()
                ),
                ("mailo/work", "cloud", want)
            );
        }
    }
}
