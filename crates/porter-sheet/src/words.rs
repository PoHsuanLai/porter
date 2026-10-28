//! The sheet's declared vocabulary: the words a person reads for the values the sheet shows. They
//! are functions over typed values of the lower crates, so those crates name no label: a program
//! that words a value differently (an app with its own copy) takes the typed value and ignores
//! these. What a daemon's settings page or the add-account sheet shows comes from here.

use crate::ProviderGroup;
use porter_capability::{Access, Capability, CapabilityKind, StorageCap, StorageScope};

/// The group as a person reads it, the heading of its part of the page.
pub fn group_heading(group: ProviderGroup) -> &'static str {
    match group {
        ProviderGroup::Internet => "Internet Accounts",
        ProviderGroup::Intelligence => "Intelligence",
        ProviderGroup::Agent => "Assistants",
    }
}

/// The kind as a person reads it: the one source for the account sheet's service names,
/// Settings' Services switches and the noun in a grant's row ("Sync can use Files"). No
/// wildcard arm, so a new kind must be named here.
pub fn service_name(kind: CapabilityKind) -> &'static str {
    match kind {
        CapabilityKind::Identity => "Account details",
        CapabilityKind::Mail => "Mail",
        CapabilityKind::Calendar => "Calendar",
        CapabilityKind::Contacts => "Contacts",
        CapabilityKind::Tasks => "Tasks",
        CapabilityKind::Notes => "Notes",
        CapabilityKind::Storage => "Files",
        CapabilityKind::Photos => "Photos",
        CapabilityKind::Llm => "Language model",
        CapabilityKind::Embeddings => "Search by meaning",
        CapabilityKind::Speech => "Speech",
        CapabilityKind::ImageGen => "Image generation",
        CapabilityKind::Rerank => "Result ranking",
        CapabilityKind::ComputerUse => "Operating windows",
        CapabilityKind::KeyValue => "Small synced items",
        CapabilityKind::Push => "Notifications",
        CapabilityKind::Agent => "Assistant",
        // A kind a later porter-capability adds: named by its own words once this crate knows it.
        _ => "Other service",
    }
}

/// What this capability lets porter do with the account, in the words a sign-in that asks
/// for it shows ("See and change this account's files"), where those say more than the
/// kind's name: today only a whole store does.
pub fn access_words(capability: &Capability) -> Option<&'static str> {
    match capability {
        Capability::Storage(StorageCap {
            scope: StorageScope::Full,
            access,
            ..
        }) => match access {
            Access::ReadWrite => Some("See and change this account's files"),
            Access::Read => Some("See this account's files"),
            Access::None => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_capability::{Delta, HashKind, Offered, QuotaReport};

    fn storage(scope: StorageScope, access: Access) -> Capability {
        Capability::Storage(StorageCap {
            access,
            delta: Delta::Poll,
            quota: QuotaReport::Reported,
            scope,
            hashes: HashKind::Sha1,
            ranges: Offered::Present,
            chunked_upload: Offered::Absent,
        })
    }

    #[test]
    fn a_service_is_named_in_the_words_of_its_page() {
        assert_eq!(service_name(CapabilityKind::Storage), "Files");
        assert_eq!(service_name(CapabilityKind::Llm), "Language model");
        assert_eq!(service_name(CapabilityKind::Identity), "Account details");
    }

    #[test]
    fn only_a_whole_store_says_what_access_it_gives() {
        let full = |access| storage(StorageScope::Full, access);
        assert_eq!(
            access_words(&full(Access::ReadWrite)),
            Some("See and change this account's files")
        );
        assert_eq!(
            access_words(&full(Access::Read)),
            Some("See this account's files")
        );
        assert_eq!(access_words(&full(Access::None)), None);
        assert_eq!(
            access_words(&storage(StorageScope::AppFolder, Access::ReadWrite)),
            None
        );
    }
}
