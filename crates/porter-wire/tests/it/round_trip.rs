//! Every request and reply survives its serde form, and the forms other programs read keep their
//! slugs.

use porter_capability::Subject;
use porter_capability::{Access, Capability, Delta, HashKind, StorageCap, StorageScope};
use porter_core::consent::Usage;
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{AccountLabel, Candidate, DataClass, ProviderHint, Restriction};
use porter_match::{Need, StorageNeed};
use porter_wire::{AccountsReply, AccountsRequest, ParentWindow, Refusal};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    let back: T = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(&back, value, "{json}");
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializes")
}

fn account_id(s: &str) -> AccountId {
    AccountId::parse(s).expect("account id")
}

fn grant_id(s: &str) -> GrantId {
    GrantId::parse(s).expect("grant id")
}

fn storage() -> Capability {
    Capability::Storage(StorageCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        quota: true,
        scope: StorageScope::Full,
        hashes: HashKind::Sha1,
        ranges: true,
        chunked_upload: false,
    })
}

fn candidate() -> Candidate {
    Candidate::new(
        account_id("cloud"),
        AccountLabel("ada@example.org".into()),
        ProviderId::parse("nextcloud").expect("provider"),
        Subject::Account,
        storage(),
        Restriction::none(),
        grant_id("g1"),
    )
}

#[test]
fn every_request_and_reply_round_trips() {
    let need = Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::Full,
        true,
    ));
    let window = ParentWindow::Handle("wayland:abc".into());
    let requests = [
        AccountsRequest::Choose {
            need: need.clone(),
            class: DataClass::Photos,
            usage: Usage::Interactive,
            window: window.clone(),
            account: None,
        },
        AccountsRequest::Choose {
            need,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            window: ParentWindow::Unparented,
            account: Some(AccountId::parse("b").expect("id")),
        },
        AccountsRequest::AddAccount {
            hint: ProviderHint::Provider(ProviderId::parse("nextcloud").expect("id")),
            window: ParentWindow::Unparented,
        },
        AccountsRequest::AddAccount {
            hint: ProviderHint::Any,
            window: window.clone(),
        },
        AccountsRequest::Reauthenticate {
            account: account_id("cloud"),
            window,
        },
    ];
    requests.iter().for_each(round_trip);
    let replies = [
        AccountsReply::Chosen(candidate()),
        AccountsReply::Added(account_id("cloud")),
        AccountsReply::AlreadyAdded(account_id("cloud")),
        AccountsReply::Reauthenticated,
        AccountsReply::Refused(Refusal::AudienceNotGranted),
        AccountsReply::Refused(Refusal::EndpointNotGranted),
    ];
    replies.iter().for_each(round_trip);
}

#[test]
fn a_refusal_keeps_its_slug() {
    assert_eq!(
        json(&AccountsReply::Refused(Refusal::EndpointNotGranted)),
        r#"{"kind":"refused","v":"endpoint_not_granted"}"#
    );
}
