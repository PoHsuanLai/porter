//! Every request and reply survives its serde form, and the forms other programs read keep their
//! slugs.

use porter_core::capability::{
    Access, Capability, CapabilityKind, Delta, HashKind, Offered, QuotaReport, StorageCap,
    StorageScope,
};
use porter_core::consent::{Availability, Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::need::{Need, StorageNeed};
use porter_core::token::Audience;
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{
    AccountLabel, AppId, AppName, Candidate, DataClass, EndpointUrl, Isolation, IssuedToken,
    ProviderHint, Restriction, SecretText, SpaceId, SpaceScope, Subject, TokenKind,
};
use porter_ids::UnixSeconds;
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

fn url() -> EndpointUrl {
    EndpointUrl::parse("imaps://imap.example.org").expect("url")
}

fn storage() -> Capability {
    Capability::Storage(StorageCap {
        access: Access::ReadWrite,
        delta: Delta::Poll,
        quota: QuotaReport::Reported,
        scope: StorageScope::Full,
        hashes: HashKind::Sha1,
        ranges: Offered::Present,
        chunked_upload: Offered::Absent,
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

fn grant() -> Grant {
    Grant {
        id: grant_id("g1"),
        key: GrantKey {
            app: AppId {
                name: AppName::parse("org.quire.Photos").expect("name"),
                isolation: Isolation::Flatpak,
            },
            account: account_id("cloud"),
            kind: CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Background,
            space: SpaceScope::Only(SpaceId::parse("work").expect("space")),
            reach: StorageScope::AppFolder,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1_790_000_000),
    }
}

#[test]
fn every_request_and_reply_round_trips() {
    let need = Need::Storage(StorageNeed::new(
        Access::ReadWrite,
        Delta::Poll,
        StorageScope::Full,
        QuotaReport::Reported,
    ));
    let window = ParentWindow::Handle("wayland:abc".into());
    let requests = vec![
        AccountsRequest::Query {
            need: need.clone(),
            class: DataClass::Photos,
            usage: Usage::Interactive,
        },
        AccountsRequest::Availability {
            need: need.clone(),
            class: DataClass::Files,
            usage: Usage::Background,
        },
        AccountsRequest::Choose {
            need,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            window: window.clone(),
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
        AccountsRequest::ListGrants,
        AccountsRequest::Revoke {
            grant: grant_id("g1"),
        },
        AccountsRequest::IssueToken {
            grant: grant_id("g1"),
            audience: Audience("imap".into()),
        },
        AccountsRequest::OpenAuthenticated {
            grant: grant_id("g1"),
            endpoint: url(),
        },
        AccountsRequest::OpenLinked {
            grant: grant_id("g1"),
            origin: url(),
        },
    ];
    requests.iter().for_each(round_trip);
    let replies = vec![
        AccountsReply::Candidates(vec![candidate()]),
        AccountsReply::Availability(Availability::Unsupported),
        AccountsReply::Chosen(candidate()),
        AccountsReply::Added(account_id("cloud")),
        AccountsReply::AlreadyAdded(account_id("cloud")),
        AccountsReply::Reauthenticated,
        AccountsReply::Grants(vec![grant()]),
        AccountsReply::Revoked,
        AccountsReply::Token(IssuedToken::new(
            TokenKind::Bearer,
            SecretText::new("t"),
            UnixSeconds(1),
        )),
        AccountsReply::Authenticated,
        AccountsReply::Refused(Refusal::AudienceNotGranted),
        AccountsReply::Refused(Refusal::EndpointNotGranted),
    ];
    replies.iter().for_each(round_trip);
}

#[test]
fn new_wire_requests_keep_their_slugs() {
    assert_eq!(
        json(&AccountsRequest::OpenAuthenticated {
            grant: grant_id("g1"),
            endpoint: url(),
        }),
        r#"{"kind":"open_authenticated","v":{"grant":"g1","endpoint":"imaps://imap.example.org"}}"#
    );
    assert_eq!(
        json(&AccountsRequest::OpenLinked {
            grant: grant_id("g1"),
            origin: url(),
        }),
        r#"{"kind":"open_linked","v":{"grant":"g1","origin":"imaps://imap.example.org"}}"#
    );
    assert_eq!(
        json(&AccountsReply::Authenticated),
        r#"{"kind":"authenticated"}"#
    );
    assert_eq!(
        json(&AccountsReply::Refused(Refusal::EndpointNotGranted)),
        r#"{"kind":"refused","v":"endpoint_not_granted"}"#
    );
}
