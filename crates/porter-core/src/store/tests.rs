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

// ---- one frozen document per vocabulary version (rel-12) ----
//
// Each text is a registry as the build at that version wrote it, spelled out here and never
// regenerated from this build: a change to how a name is spelled must break the version that used
// the old spelling. What each adds over the one before is what that version's bump introduced:
// 4 the `sieve` family, 5 the agent account (`agent_login`, `needs_login`), 6 the `tasks` kind
// and class, 7 the seven-day sign-in with its date, 8 the `session` grant scope. 9 and 10 changed
// the sheet's wire only, so their documents are the 8 one with the number moved.

const AT_THREE: &str = r#"{
  "vocab": 3,
  "accounts": [
    {"id": "cloud", "provider": "nextcloud", "label": "ada@example.org", "state": "ok", "auth": "login_flow_v2", "capabilities": [{"subject": {"kind": "account"}, "offer": {"kind": "absent", "v": {"kind": "notes", "reason": "not_on_server"}}, "provenance": "probed"}], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": [{"family": "webdav", "url": "https://cloud.example.org/remote.php/dav/", "tls": "implicit", "login": "ada"}]},
    {"id": "mail", "provider": "generic-imap", "label": "ada@example.org", "state": "ok", "auth": "password", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": [{"family": "imap", "url": "imaps://imap.example.org:993", "tls": "implicit", "login": "ada"}]}
  ],
  "grants": [
    {"id": "g-once", "key": {"app": {"name": "org.quire.Photos", "isolation": "flatpak"}, "account": "cloud", "kind": "storage", "class": "photos", "usage": "interactive", "space": {"kind": "any"}}, "decision": "allow", "scope": "once", "at": 1790000000}
  ],
  "toggles": [{"account": "cloud", "kind": "notes", "toggle": "off"}]
}"#;

const AT_FOUR: &str = r#"{
  "vocab": 4,
  "accounts": [
    {"id": "cloud", "provider": "nextcloud", "label": "ada@example.org", "state": "ok", "auth": "login_flow_v2", "capabilities": [{"subject": {"kind": "account"}, "offer": {"kind": "absent", "v": {"kind": "notes", "reason": "not_on_server"}}, "provenance": "probed"}], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": [{"family": "webdav", "url": "https://cloud.example.org/remote.php/dav/", "tls": "implicit", "login": "ada"}]},
    {"id": "mail", "provider": "generic-imap", "label": "ada@example.org", "state": "ok", "auth": "password", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": [{"family": "imap", "url": "imaps://imap.example.org:993", "tls": "implicit", "login": "ada"}, {"family": "sieve", "url": "sieve://imap.example.org:4190", "tls": "start_tls", "login": "ada"}]}
  ],
  "grants": [
    {"id": "g-once", "key": {"app": {"name": "org.quire.Photos", "isolation": "flatpak"}, "account": "cloud", "kind": "storage", "class": "photos", "usage": "interactive", "space": {"kind": "any"}}, "decision": "allow", "scope": "once", "at": 1790000000}
  ],
  "toggles": [{"account": "cloud", "kind": "notes", "toggle": "off"}]
}"#;

const AT_FIVE: &str = r#"{
  "vocab": 5,
  "accounts": [
    {"id": "cloud", "provider": "nextcloud", "label": "ada@example.org", "state": "ok", "auth": "login_flow_v2", "capabilities": [{"subject": {"kind": "account"}, "offer": {"kind": "absent", "v": {"kind": "notes", "reason": "not_on_server"}}, "provenance": "probed"}], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": [{"family": "webdav", "url": "https://cloud.example.org/remote.php/dav/", "tls": "implicit", "login": "ada"}]},
    {"id": "claude-code", "provider": "claude-code", "label": "Claude Code", "state": "needs_login", "auth": "agent_login", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": []}
  ],
  "grants": [
    {"id": "g-once", "key": {"app": {"name": "org.quire.Photos", "isolation": "flatpak"}, "account": "cloud", "kind": "storage", "class": "photos", "usage": "interactive", "space": {"kind": "any"}}, "decision": "allow", "scope": "once", "at": 1790000000}
  ],
  "toggles": [{"account": "cloud", "kind": "notes", "toggle": "off"}]
}"#;

const AT_SIX: &str = r#"{
  "vocab": 6,
  "accounts": [
    {"id": "cloud", "provider": "nextcloud", "label": "ada@example.org", "state": "ok", "auth": "login_flow_v2", "capabilities": [{"subject": {"kind": "account"}, "offer": {"kind": "absent", "v": {"kind": "notes", "reason": "not_on_server"}}, "provenance": "probed"}], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": [{"family": "webdav", "url": "https://cloud.example.org/remote.php/dav/", "tls": "implicit", "login": "ada"}]}
  ],
  "grants": [
    {"id": "g-once", "key": {"app": {"name": "org.quire.Photos", "isolation": "flatpak"}, "account": "cloud", "kind": "storage", "class": "photos", "usage": "interactive", "space": {"kind": "any"}}, "decision": "allow", "scope": "once", "at": 1790000000},
    {"id": "g-tasks", "key": {"app": {"name": "org.quire.Tasks", "isolation": "flatpak"}, "account": "cloud", "kind": "tasks", "class": "tasks", "usage": "interactive", "space": {"kind": "any"}}, "decision": "allow", "scope": "always", "at": 1790000002}
  ],
  "toggles": []
}"#;

const AT_SEVEN_RICH: &str = r#"{
  "vocab": 7,
  "accounts": [
    {"id": "google-ada", "provider": "google", "label": "ada@example.org", "state": "needs_reauth", "auth": "oauth_pkce", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "seven_days", "consent": "user", "limits": [], "signed_in": 1700000000}, "endpoints": []}
  ],
  "grants": [
    {"id": "g-always", "key": {"app": {"name": "org.quire.Agent.claude-code", "isolation": "unsandboxed"}, "account": "google-ada", "kind": "llm", "class": "prompt", "usage": "background", "space": {"kind": "any"}}, "decision": "deny", "scope": "always", "at": 1790000001}
  ],
  "toggles": []
}"#;

const AT_EIGHT: &str = r#"{
  "vocab": 8,
  "accounts": [
    {"id": "claude-code", "provider": "claude-code", "label": "Claude Code", "state": "ok", "auth": "agent_login", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": []}
  ],
  "grants": [
    {"id": "g-session", "key": {"app": {"name": "org.quire.Agent.claude-code", "isolation": "unsandboxed"}, "account": "claude-code", "kind": "llm", "class": "prompt", "usage": "background", "space": {"kind": "any"}}, "decision": "allow", "scope": {"session": "sess-1"}, "at": 1790000003}
  ],
  "toggles": []
}"#;

const AT_NINE: &str = r#"{
  "vocab": 9,
  "accounts": [
    {"id": "claude-code", "provider": "claude-code", "label": "Claude Code", "state": "ok", "auth": "agent_login", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": []}
  ],
  "grants": [
    {"id": "g-session", "key": {"app": {"name": "org.quire.Agent.claude-code", "isolation": "unsandboxed"}, "account": "claude-code", "kind": "llm", "class": "prompt", "usage": "background", "space": {"kind": "any"}}, "decision": "allow", "scope": {"session": "sess-1"}, "at": 1790000003}
  ],
  "toggles": []
}"#;

const AT_TEN: &str = r#"{
  "vocab": 10,
  "accounts": [
    {"id": "claude-code", "provider": "claude-code", "label": "Claude Code", "state": "ok", "auth": "agent_login", "capabilities": [], "restriction": {"verification": {"kind": "not_needed"}, "token_lifetime": "standard", "consent": "user", "limits": []}, "endpoints": []}
  ],
  "grants": [
    {"id": "g-session", "key": {"app": {"name": "org.quire.Agent.claude-code", "isolation": "unsandboxed"}, "account": "claude-code", "kind": "llm", "class": "prompt", "usage": "background", "space": {"kind": "any"}}, "decision": "allow", "scope": {"session": "sess-1"}, "at": 1790000003}
  ],
  "toggles": []
}"#;

/// Every frozen document reads, whatever version wrote it, as the current vocabulary, with what
/// that version could hold: the rows say what each is expected to come out as.
#[test]
fn a_document_frozen_at_each_vocabulary_since_the_first_stored_one_reads() {
    // (version, text, account ids, grant ids)
    let table: [(u16, &str, &[&str], &[&str]); 8] = [
        (3, AT_THREE, &["cloud", "mail"], &["g-once"]),
        (4, AT_FOUR, &["cloud", "mail"], &["g-once"]),
        (5, AT_FIVE, &["cloud", "claude-code"], &["g-once"]),
        (6, AT_SIX, &["cloud"], &["g-once", "g-tasks"]),
        (7, AT_SEVEN_RICH, &["google-ada"], &["g-always"]),
        (8, AT_EIGHT, &["claude-code"], &["g-session"]),
        (9, AT_NINE, &["claude-code"], &["g-session"]),
        (10, AT_TEN, &["claude-code"], &["g-session"]),
    ];
    // One row per version since the first stored, none missing and none repeated.
    let versions: Vec<u16> = table.iter().map(|row| row.0).collect();
    assert_eq!(
        versions,
        (FIRST_PERSISTED.0..=VocabVersion::CURRENT.0).collect::<Vec<_>>(),
        "a vocabulary bump adds its frozen document here"
    );
    for (version, text, accounts, grants) in table {
        assert!(
            text.contains(&format!("\"vocab\": {version},")),
            "{version}"
        );
        let read = Persisted::from_json(text)
            .unwrap_or_else(|why| panic!("the vocabulary {version} document: {why}"));
        assert_eq!(read.vocab, VocabVersion::CURRENT, "{version}");
        let got: Vec<_> = read.accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(got, accounts, "{version}: accounts");
        let got: Vec<_> = read.grants.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(got, grants, "{version}: grants");
        // What is read is what is written next: the document survives its own round trip.
        let again = read.to_json().expect("json");
        assert_eq!(
            Persisted::from_json(&again),
            Ok(read),
            "{version}: round trip"
        );
    }
}

/// What each version's bump added reads as itself in the frozen documents.
#[test]
fn what_a_version_added_reads_as_what_it_was() {
    let four = Persisted::from_json(AT_FOUR).expect("four");
    assert_eq!(four.accounts[1].endpoints[1].family, Family::Sieve);
    assert_eq!(four.accounts[1].endpoints[1].tls, Tls::StartTls);
    let five = Persisted::from_json(AT_FIVE).expect("five");
    assert_eq!(five.accounts[1].auth, AuthKind::AgentLogin);
    assert_eq!(five.accounts[1].state, AccountState::NeedsLogin);
    let six = Persisted::from_json(AT_SIX).expect("six");
    assert_eq!(six.grants[1].key.kind, CapabilityKind::Tasks);
    assert_eq!(six.grants[1].key.class, DataClass::Tasks);
    let seven = Persisted::from_json(AT_SEVEN_RICH).expect("seven");
    assert_eq!(
        seven.accounts[0].restriction.token_lifetime,
        TokenLifetime::SevenDays
    );
    assert_eq!(
        seven.accounts[0].restriction.signed_in,
        Some(UnixSeconds(1_700_000_000))
    );
    assert_eq!(seven.accounts[0].state, AccountState::NeedsReauth);
    let eight = Persisted::from_json(AT_EIGHT).expect("eight");
    assert_eq!(
        eight.grants[0].scope,
        GrantScope::Session(LauncherSession::parse("sess-1").expect("id"))
    );
}

#[test]
fn a_document_stored_at_vocabulary_ten_is_read_unchanged() {
    // The fixture is what the build before `ProviderRow::group` wrote.
    let at_ten = filled().to_json().expect("json").replacen(
        &format!("\"vocab\": {}", VocabVersion::CURRENT.0),
        "\"vocab\": 10",
        1,
    );
    assert!(at_ten.contains("\"vocab\": 10"));
    assert_eq!(Persisted::from_json(&at_ten), Ok(filled()));
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
