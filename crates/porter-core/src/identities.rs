//! The identities signed in on this machine: the registry's accounts of a sign-in kind (OAuth,
//! a password, an app password), as an app sees them when it offers "Continue as". An identity
//! is never a token or a secret: it is an id, an address, a provider and a kind. Local runtimes,
//! Tailscale, AI keys and agent logins are accounts but not identities.

use crate::account::Account;
use crate::{AccountId, ProviderId};
use porter_ids::slug_id;
use serde::{Deserialize, Serialize};

slug_id!(
    /// One identity signed in on this machine. It is the id of the registry account it is, and a
    /// separate type so an app holds an identity without holding an account's powers.
    IdentityId,
    "identity id"
);

impl IdentityId {
    /// The identity that `account` is.
    pub fn of(account: &AccountId) -> Self {
        Self(account.as_str().to_owned())
    }

    /// Whether this is the identity that `account` is.
    pub fn is(&self, account: &AccountId) -> bool {
        self.0 == account.as_str()
    }
}

/// The address an identity is shown by (`ada@example.org`): what the account is labelled with.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityAddress(pub String);

/// What a person calls an identity's provider ("Google"), as the provider file labels it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdentityName(pub String);

/// How an identity signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IdentityKind {
    /// In the browser, with OAuth: the app never gets more than the person's pick.
    #[serde(rename = "oauth")]
    OAuth,
    /// With a password or an app password, which the person's pick may hand to the app
    /// (`Identities1.TakePassword`).
    Password,
}

/// One identity, as `Identities1.List` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Identity {
    /// Its id.
    pub id: IdentityId,
    /// The address it is shown by.
    pub address: IdentityAddress,
    /// The provider file it was made from.
    pub provider: ProviderId,
    /// What a person calls the provider.
    pub name: IdentityName,
    /// How it signs in.
    pub kind: IdentityKind,
}

impl Identity {
    /// An identity with these parts.
    pub fn new(
        id: IdentityId,
        address: IdentityAddress,
        provider: ProviderId,
        name: IdentityName,
        kind: IdentityKind,
    ) -> Self {
        Self {
            id,
            address,
            provider,
            name,
            kind,
        }
    }

    /// The identity `account` is, or `None` when it is not one (see [`crate::AuthKind::facts`]).
    /// `provider_name` is what the provider file labels the provider; without it the provider's
    /// id stands in.
    pub fn of(account: &Account, provider_name: Option<&str>) -> Option<Self> {
        let kind = account.auth.facts().identity?;
        let name = provider_name.unwrap_or(account.provider.as_str());
        Some(Self::new(
            IdentityId::of(&account.id),
            IdentityAddress(account.label.0.clone()),
            account.provider.clone(),
            IdentityName(name.to_owned()),
            kind,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_ids::word::{from_word, word as word_of};

    #[test]
    fn a_kind_reads_back_from_its_word_and_an_identity_from_its_account() {
        for (kind, word) in [
            (IdentityKind::OAuth, "oauth"),
            (IdentityKind::Password, "password"),
        ] {
            assert_eq!(word_of(&kind), word);
            assert_eq!(from_word::<IdentityKind>(word), Some(kind));
        }
        assert_eq!(from_word::<IdentityKind>("OAuth"), None);
        let account = AccountId::parse("ada.example").expect("id");
        let id = IdentityId::of(&account);
        assert!(id.is(&account));
        assert!(!id.is(&AccountId::parse("other").expect("id")));
        assert_eq!(IdentityId::parse(id.as_str()), Ok(id));
    }
}
