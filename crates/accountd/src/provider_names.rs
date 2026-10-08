//! What the settings module says of an account's provider: its label ("Fastmail") and the part
//! of the Accounts page it is listed under, both read from the provider files accountd loaded.
//! An account whose provider file has gone keeps its place by the group
//! [`ProviderGroup::fallback`] decides from how it signs in and what it offers, and shows the
//! provider's id where a label would be.

use porter_core::sheet::ProviderGroup;
use porter_core::{Account, ProviderId};
use porter_provider::ProviderSpec;
use std::collections::BTreeMap;

/// The label and group of each loaded provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderNames {
    known: BTreeMap<ProviderId, (String, ProviderGroup)>,
}

impl ProviderNames {
    /// The names of `specs`, one per provider id.
    pub fn from_specs(specs: &[ProviderSpec]) -> Self {
        Self {
            known: specs
                .iter()
                .map(|spec| (spec.id.clone(), (spec.label.clone(), spec.group())))
                .collect(),
        }
    }

    /// What a person reads for `provider`, if its file is loaded.
    pub fn label_of(&self, provider: &ProviderId) -> Option<&str> {
        self.known.get(provider).map(|(label, _)| label.as_str())
    }

    /// The group `account` is listed under: its provider's, else the one decided from the
    /// account itself.
    pub fn group_of(&self, account: &Account) -> ProviderGroup {
        match self.known.get(&account.provider) {
            Some((_, group)) => *group,
            None => {
                let kinds: Vec<_> = account
                    .capabilities
                    .iter()
                    .map(|c| c.offer.kind())
                    .collect();
                ProviderGroup::fallback(account.auth, &kinds)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::AuthKind;
    use porter_fake::{mail_account, storage_account};
    use porter_provider::parse_provider;

    fn names() -> ProviderNames {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../providers/fastmail.toml"
        ))
        .expect("fastmail ships");
        ProviderNames::from_specs(&[parse_provider(&text).expect("parses")])
    }

    #[test]
    fn a_loaded_provider_gives_its_label_and_its_files_group() {
        let mut account = mail_account();
        account.provider = ProviderId::parse("fastmail").expect("id");
        assert_eq!(names().label_of(&account.provider), Some("Fastmail"));
        assert_eq!(names().group_of(&account), ProviderGroup::Internet);
    }

    #[test]
    fn an_account_whose_file_is_gone_is_placed_by_how_it_signs_in() {
        let mut account = storage_account();
        account.provider = ProviderId::parse("gone").expect("id");
        assert_eq!(names().label_of(&account.provider), None);
        let table = [
            (AuthKind::AgentLogin, ProviderGroup::Agent),
            (AuthKind::LocalRuntime, ProviderGroup::Intelligence),
            (AuthKind::Password, ProviderGroup::Internet),
            (AuthKind::OAuthPkce, ProviderGroup::Internet),
        ];
        for (auth, want) in table {
            account.auth = auth;
            assert_eq!(names().group_of(&account), want, "{auth:?}");
        }
    }
}
