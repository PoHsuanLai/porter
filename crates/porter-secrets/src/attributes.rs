//! How a [`SecretKey`] is written as Secret Service attributes: `{service, account, purpose}`.

use porter_core::SecretKey;

/// The `service` attribute of every item porter files.
pub const SERVICE: &str = "porter";

/// The attributes of one item, in the order the store is searched by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretAttributes {
    /// Always [`SERVICE`].
    pub service: &'static str,
    /// The account id.
    pub account: String,
    /// The purpose in its serde form, which is its stored schema.
    pub purpose: String,
}

/// The attributes `key` is filed under.
pub fn attributes(key: &SecretKey) -> SecretAttributes {
    SecretAttributes {
        service: SERVICE,
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
        for (purpose, want) in cases {
            let key = SecretKey {
                account: AccountId::parse("cloud").expect("id"),
                purpose,
            };
            let got = attributes(&key);
            assert_eq!(
                (got.service, got.account.as_str(), got.purpose.as_str()),
                ("porter", "cloud", want)
            );
        }
    }
}
