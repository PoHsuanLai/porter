use super::*;
use crate::app_id::{AppName, Isolation};
use crate::consent::{Decision, GrantKey, GrantScope, Usage};
use crate::endpoint::{LoginName, ServiceEndpoint, Tls};
use crate::launcher_session::LauncherSession;
use crate::offer::{Claim, Offer, Provenance, Subject};
use crate::restriction::{Restriction, TokenLifetime};
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
    for from in (FIRST_PERSISTED.0..).take_while(|v| *v < VocabVersion::CURRENT.0) {
        assert!(
            MIGRATIONS.iter().any(|(start, _)| start.0 == from),
            "vocabulary {from} to {} has no migration row; add one and a fixture of the old \
             document to these tests",
            from + 1
        );
    }
}

#[test]
fn a_document_stored_at_vocabulary_three_is_read_unchanged() {
    // The fixture is what the build before the `sieve` family wrote.
    let at_three = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 3",
        1,
    );
    assert!(at_three.contains("\"vocab\": 3"));
    assert_eq!(Persisted::from_json(&at_three), Ok(filled()));
}

#[test]
fn a_document_stored_at_vocabulary_four_is_read_unchanged() {
    // The fixture is what the build before the `agent` kind wrote.
    let at_four = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 4",
        1,
    );
    assert!(at_four.contains("\"vocab\": 4"));
    assert_eq!(Persisted::from_json(&at_four), Ok(filled()));
}

#[test]
fn a_document_stored_at_vocabulary_five_is_read_unchanged_with_the_sign_in_age_unknown() {
    // The fixture is what the build before `Restriction::signed_in` wrote: a Google account in
    // testing, whose restriction has no `signed_in`.
    let mut registry = filled();
    registry.accounts[0].restriction.token_lifetime = TokenLifetime::SevenDays;
    let at_five = registry.to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 5",
        1,
    );
    assert!(at_five.contains("\"vocab\": 5") && !at_five.contains("signed_in"));
    assert!(at_five.contains("\"seven_days\""));
    assert_eq!(Persisted::from_json(&at_five), Ok(registry));
}

#[test]
fn a_document_stored_at_vocabulary_six_is_read_unchanged() {
    // The fixture is what the build before `Restriction::signed_in` and the sheet rows wrote.
    let at_six = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 6",
        1,
    );
    assert!(at_six.contains("\"vocab\": 6"));
    assert_eq!(Persisted::from_json(&at_six), Ok(filled()));
}

/// A document exactly as the build at vocabulary 7 wrote it: no account, two grants of the two
/// scopes it knew. Frozen text, not generated by this build.
const AT_SEVEN: &str = r#"{
  "vocab": 7,
  "accounts": [],
  "grants": [
    {"id": "g-once", "key": {"app": {"name": "org.quire.Photos", "isolation": "flatpak"}, "account": "cloud", "kind": "storage", "class": "photos", "usage": "interactive", "space": {"kind": "any"}}, "decision": "allow", "scope": "once", "at": 1790000000},
    {"id": "g-always", "key": {"app": {"name": "org.quire.Agent.claude-code", "isolation": "unsandboxed"}, "account": "cloud", "kind": "llm", "class": "prompt", "usage": "background", "space": {"kind": "any"}}, "decision": "deny", "scope": "always", "at": 1790000001}
  ],
  "toggles": []
}"#;

#[test]
fn a_document_stored_at_vocabulary_seven_is_read_unchanged_and_its_scopes_still_read() {
    // The fixture is what the build before `GrantScope::Session` wrote.
    let read = Persisted::from_json(AT_SEVEN).expect("a vocabulary 7 document reads");
    assert_eq!(read.vocab, VocabVersion::CURRENT);
    let scopes: Vec<_> = read.grants.iter().map(|g| g.scope.clone()).collect();
    assert_eq!(scopes, [GrantScope::Once, GrantScope::Always]);
    assert_eq!(read.grants[0].id.as_str(), "g-once");
    assert_eq!(read.grants[1].decision, Decision::Deny);
}

#[test]
fn a_document_stored_at_vocabulary_eight_is_read_unchanged() {
    // The fixture is what the build before `SignInFault::AlreadyAdded` and the app labels wrote.
    let at_eight = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 8",
        1,
    );
    assert!(at_eight.contains("\"vocab\": 8"));
    assert_eq!(Persisted::from_json(&at_eight), Ok(filled()));
}

#[test]
fn a_session_grant_survives_the_document_and_keeps_its_form() {
    let mut registry = filled();
    registry.grants[0].scope = GrantScope::Session(LauncherSession::parse("sess-1").expect("id"));
    let json = registry.to_json().expect("json");
    let document: Value = serde_json::from_str(&json).expect("value");
    assert_eq!(
        document["grants"][0]["scope"],
        serde_json::json!({"session": "sess-1"})
    );
    assert_eq!(Persisted::from_json(&json), Ok(registry));
    // A session id that is not an id does not read: the file is refused, not guessed at.
    let bad = json.replace("sess-1", "Sess 1");
    assert!(matches!(
        Persisted::from_json(&bad),
        Err(StoreFault::Unreadable(_))
    ));
}

#[test]
fn a_sign_in_date_survives_the_document() {
    let mut registry = filled();
    registry.accounts[0].restriction.token_lifetime = TokenLifetime::SevenDays;
    registry.accounts[0].restriction.signed_in = Some(UnixSeconds(1_700_000_000));
    let json = registry.to_json().expect("json");
    assert!(json.contains("\"signed_in\": 1700000000"));
    assert_eq!(Persisted::from_json(&json), Ok(registry));
}

#[test]
fn an_agent_login_account_survives_its_document_and_keeps_its_slugs() {
    let mut registry = filled();
    registry.accounts.push(Account {
        id: AccountId::parse("claude-code").expect("id"),
        provider: ProviderId::parse("claude-code").expect("id"),
        label: AccountLabel("Claude Code".into()),
        state: AccountState::NeedsLogin,
        auth: AuthKind::AgentLogin,
        capabilities: Vec::new(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    });
    let json = registry.to_json().expect("json");
    let document: Value = serde_json::from_str(&json).expect("value");
    assert_eq!(document["accounts"][1]["state"], "needs_login");
    assert_eq!(document["accounts"][1]["auth"], "agent_login");
    assert_eq!(Persisted::from_json(&json), Ok(registry));
}

#[test]
fn a_document_stored_at_vocabulary_five_is_read_unchanged() {
    // The fixture is what the build before the `tasks` data class wrote.
    let at_five = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 5",
        1,
    );
    assert!(at_five.contains("\"vocab\": 5"));
    assert_eq!(Persisted::from_json(&at_five), Ok(filled()));
}
