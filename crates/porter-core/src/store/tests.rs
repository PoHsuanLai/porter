use super::*;
use crate::app_id::{AppName, Isolation};
use crate::consent::{Decision, GrantKey, GrantScope, Usage};
use crate::endpoint::{LoginName, ServiceEndpoint, Tls};
use crate::offer::{Claim, Offer, Provenance, Subject};
use crate::restriction::Restriction;
use crate::space::SpaceScope;
use crate::units::UnixSeconds;
use crate::{
    AccountLabel, AccountState, AppId, AuthKind, DataClass, EndpointUrl, Family, GrantId,
    ProviderId,
};

fn account(endpoints: Vec<ServiceEndpoint>) -> Account {
    Account {
        id: AccountId::parse("cloud").expect("id"),
        provider: ProviderId::parse("nextcloud").expect("id"),
        label: AccountLabel("ada@example.org".into()),
        state: AccountState::Ok,
        auth: AuthKind::LoginFlowV2,
        capabilities: vec![Claim {
            subject: Subject::Account,
            offer: Offer::Absent {
                kind: CapabilityKind::Notes,
                reason: crate::AbsentReason::NotOnServer,
            },
            provenance: Provenance::Probed,
        }],
        restriction: Restriction::none(),
        endpoints,
    }
}

fn webdav(url: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        family: Family::WebDav,
        url: EndpointUrl::parse(url).expect("url"),
        tls: Tls::Implicit,
        login: LoginName("ada".into()),
    }
}

fn filled() -> Persisted {
    Persisted {
        vocab: VocabVersion::CURRENT,
        accounts: vec![account(vec![webdav(
            "https://cloud.example.org/remote.php/dav/",
        )])],
        grants: vec![Grant {
            id: GrantId::parse("g1").expect("id"),
            key: GrantKey {
                app: AppId {
                    name: AppName::parse("org.quire.Photos").expect("name"),
                    isolation: Isolation::Flatpak,
                },
                account: AccountId::parse("cloud").expect("id"),
                kind: CapabilityKind::Storage,
                class: DataClass::Photos,
                usage: Usage::Interactive,
                space: SpaceScope::Any,
            },
            decision: Decision::Allow,
            scope: GrantScope::Always,
            at: UnixSeconds(1_790_000_000),
        }],
        toggles: vec![AccountToggle {
            account: AccountId::parse("cloud").expect("id"),
            kind: CapabilityKind::Notes,
            toggle: Toggle::Off,
        }],
    }
}

#[test]
fn a_registry_survives_its_document() {
    let before = filled();
    let json = before.to_json().expect("json");
    assert_eq!(Persisted::from_json(&json).expect("read"), before);
    assert_eq!(
        Persisted::from_json(&Persisted::empty().to_json().expect("json")),
        Ok(Persisted::empty())
    );
}

#[test]
fn the_document_names_its_vocabulary_and_keeps_its_slugs() {
    let json = filled().to_json().expect("json");
    let document: Value = serde_json::from_str(&json).expect("value");
    assert_eq!(document["vocab"], Value::from(VocabVersion::CURRENT.0));
    assert_eq!(
        document["toggles"][0],
        serde_json::json!({"account": "cloud", "kind": "notes", "toggle": "off"})
    );
    assert_eq!(
        document["accounts"][0]["endpoints"][0],
        serde_json::json!({
            "family": "webdav",
            "url": "https://cloud.example.org/remote.php/dav/",
            "tls": "implicit",
            "login": "ada"
        })
    );
}

#[test]
fn a_file_that_cannot_be_read_is_refused_not_emptied() {
    let newer = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0 + 1),
        1,
    );
    let older = newer.replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0 + 1),
        &format!("\"vocab\": {}", FIRST_PERSISTED.0 - 1),
        1,
    );
    let incoherent = filled()
        .to_json()
        .expect("json")
        .replace("implicit", "start_tls");
    let cases: Vec<(&str, String, StoreFault)> = vec![
        (
            "not json",
            "{".into(),
            StoreFault::Unreadable("EOF while parsing an object at line 1 column 1".into()),
        ),
        (
            "no vocab",
            "{}".into(),
            StoreFault::Unreadable("no vocab".into()),
        ),
        (
            "from a newer build",
            newer,
            StoreFault::FromNewerVocabulary(VocabVersion(VocabVersion::CURRENT.0 + 1)),
        ),
        (
            "from before anything was stored",
            older,
            StoreFault::NoMigration(VocabVersion(FIRST_PERSISTED.0 - 1)),
        ),
        (
            "an endpoint that does not go with its scheme",
            incoherent,
            StoreFault::BadEndpoint(AccountId::parse("cloud").expect("id")),
        ),
    ];
    for (name, text, fault) in cases {
        assert_eq!(Persisted::from_json(&text), Err(fault), "{name}");
    }
}

#[test]
fn every_version_since_the_first_stored_one_has_a_migration() {
    for from in FIRST_PERSISTED.0..VocabVersion::CURRENT.0 {
        assert!(
            MIGRATIONS.iter().any(|(start, _)| start.0 == from),
            "vocabulary {from} to {} has no migration row; add one and a fixture of the old \
             document to these tests",
            from + 1
        );
    }
}
