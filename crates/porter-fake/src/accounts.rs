//! Three accounts made from the fake providers, their capabilities as declared.

use crate::provider::{FakeProvider, cloud_provider, llm_provider, mail_provider};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, Claim, Offer, Provenance, Restriction, Subject,
};
use porter_provider::Provider;

fn account(id: &str, label: &str, provider: FakeProvider) -> Account {
    let spec = provider.spec();
    Account {
        id: AccountId::parse(id).unwrap_or_else(|e| panic!("fake account id: {e}")),
        provider: spec.id.clone(),
        label: AccountLabel(label.into()),
        state: AccountState::Ok,
        auth: spec.auth.kind,
        capabilities: spec
            .capabilities
            .iter()
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            })
            .collect(),
        restriction: Restriction::none(),
    }
}

/// `fake-storage`: files (read-write, poll, full) and calendars.
pub fn storage_account() -> Account {
    account("fake-storage", "ada@cloud.invalid", cloud_provider())
}

/// `fake-mail`: IMAP mail with push and sending.
pub fn mail_account() -> Account {
    account("fake-mail", "ada@mail.invalid", mail_provider())
}

/// `fake-llm`: a local chat model.
pub fn llm_account() -> Account {
    account("fake-llm", "Fake Runtime on this computer", llm_provider())
}
