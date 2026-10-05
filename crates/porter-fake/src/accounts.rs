//! Three accounts made from the fake providers, their capabilities as declared.

use crate::provider::{FakeProvider, cloud_provider, llm_provider, mail_provider};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, Claim, EndpointUrl, Family, LoginName, Offer,
    Provenance, Restriction, ServiceEndpoint, Subject, Tls,
};
use porter_provider::Provider;

fn endpoint(family: Family, url: &str, tls: Tls, login: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        family,
        url: EndpointUrl::parse(url).unwrap_or_else(|e| panic!("fake endpoint: {e}")),
        tls,
        login: LoginName(login.into()),
    }
}

fn account(
    id: &str,
    label: &str,
    provider: FakeProvider,
    endpoints: Vec<ServiceEndpoint>,
) -> Account {
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
        endpoints,
    }
}

/// `fake-storage`: files (read-write, poll, full) and calendars, at one WebDAV and one CalDAV
/// root.
pub fn storage_account() -> Account {
    let endpoints = vec![
        endpoint(
            Family::WebDav,
            "https://cloud.invalid/remote.php/dav/files/ada/",
            Tls::Implicit,
            "ada",
        ),
        endpoint(
            Family::CalDav,
            "https://cloud.invalid/remote.php/dav/calendars/ada/",
            Tls::Implicit,
            "ada",
        ),
    ];
    account(
        "fake-storage",
        "ada@cloud.invalid",
        cloud_provider(),
        endpoints,
    )
}

/// `fake-mail`: IMAP mail with push and sending, at an IMAP and an SMTP host.
pub fn mail_account() -> Account {
    let endpoints = vec![
        endpoint(
            Family::Imap,
            "imaps://imap.mail.invalid",
            Tls::Implicit,
            "ada@mail.invalid",
        ),
        endpoint(
            Family::Smtp,
            "smtp://smtp.mail.invalid:587",
            Tls::StartTls,
            "ada@mail.invalid",
        ),
    ];
    account("fake-mail", "ada@mail.invalid", mail_provider(), endpoints)
}

/// `fake-llm`: a local chat model, which has no endpoint.
pub fn llm_account() -> Account {
    account(
        "fake-llm",
        "Fake Runtime on this computer",
        llm_provider(),
        Vec::new(),
    )
}
