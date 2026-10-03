//! The frozen shapes of prov: every wire type survives its serde form, the pinned JSON of the
//! forms other programs read, the ordering the policy tables rely on, and the redaction.

use prov::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) -> String {
    let json = serde_json::to_string(value).expect("serializes");
    assert_eq!(
        &serde_json::from_str::<T>(&json).expect("deserializes"),
        value,
        "{json}"
    );
    json
}

fn app(name: &str) -> AppName {
    AppName::parse(name).expect("app name")
}

fn space(id: &str) -> SpaceId {
    SpaceId::parse(id).expect("space id")
}

fn run() -> RunId {
    RunId::parse("r-7").expect("run id")
}

fn every_actor() -> Vec<Actor> {
    let session = SessionId::parse("s-1").expect("session id");
    vec![
        Actor::User {
            via: app("org.quire.Shell"),
        },
        Actor::Companion {
            session: session.clone(),
            role: AgentRole::Planner,
        },
        Actor::Companion {
            session: session.clone(),
            role: AgentRole::Reader,
        },
        Actor::Companion {
            session,
            role: AgentRole::Cua { run: run() },
        },
        Actor::Companion {
            session: SessionId::parse("s-2").expect("session id"),
            role: AgentRole::Worker {
                task: TaskId::parse("t-4").expect("task id"),
            },
        },
        Actor::Mcp {
            client: ClientName::parse("Claude Desktop").expect("client"),
        },
        Actor::Cli,
        Actor::App {
            app: app("org.quire.Mail"),
        },
        Actor::ThirdParty {
            app: app("org.mozilla.firefox"),
            channel: Channel::Atspi,
        },
        Actor::System {
            part: SystemPart::Router,
        },
        Actor::Unknown,
    ]
}

#[test]
fn actors_round_trip_and_pin_their_json() {
    every_actor().iter().for_each(|a| {
        round_trip(a);
    });
    let cua = Actor::Companion {
        session: SessionId::parse("s-1").expect("id"),
        role: AgentRole::Cua { run: run() },
    };
    assert_eq!(
        round_trip(&cua),
        r#"{"kind":"companion","v":{"session":"s-1","role":{"kind":"cua","v":{"run":"r-7"}}}}"#
    );
    assert_eq!(round_trip(&Actor::Unknown), r#"{"kind":"unknown"}"#);
    assert_eq!(round_trip(&Actor::Cli), r#"{"kind":"cli"}"#);
    let worker = Actor::Companion {
        session: SessionId::parse("s-2").expect("id"),
        role: AgentRole::Worker {
            task: TaskId::parse("t-4").expect("id"),
        },
    };
    assert_eq!(
        round_trip(&worker),
        r#"{"kind":"companion","v":{"session":"s-2","role":{"kind":"worker","v":{"task":"t-4"}}}}"#
    );
    assert_eq!(worker.kind(), ActorKind::Companion);
}

#[test]
fn every_actor_has_a_kind() {
    let kinds: Vec<ActorKind> = every_actor().iter().map(Actor::kind).collect();
    assert_eq!(
        kinds,
        [
            ActorKind::User,
            ActorKind::Companion,
            ActorKind::Companion,
            ActorKind::Cua,
            ActorKind::Companion,
            ActorKind::Mcp,
            ActorKind::Cli,
            ActorKind::App,
            ActorKind::ThirdParty,
            ActorKind::System,
            ActorKind::Unknown,
        ]
    );
}

#[test]
fn effects_are_ordered_by_severity_and_keep_their_slugs() {
    let ordered = [
        Effect::Read,
        Effect::UndoableWrite,
        Effect::Outbound,
        Effect::Destructive,
    ];
    assert!(ordered.windows(2).all(|pair| pair[0] < pair[1]));
    let slugs: Vec<String> = ordered.iter().map(round_trip).collect();
    assert_eq!(
        slugs,
        [
            "\"read\"",
            "\"undoable_write\"",
            "\"outbound\"",
            "\"destructive\""
        ]
    );
}

fn label() -> Label {
    Label {
        integrity: Integrity::Untrusted,
        confidentiality: Confidentiality::Private([space("work"), space("home")].into()),
        classes: [DataClass::Mail, DataClass::Screen].into(),
        sources: [
            Source::Mail,
            Source::Screen {
                app: app("org.quire.Mail"),
            },
            Source::Model(ModelRole::Reader),
            Source::Mcp(ClientName::parse("cli").expect("client")),
            Source::App(app("org.quire.Calendar")),
            Source::User,
        ]
        .into(),
    }
}

#[test]
fn labels_round_trip_and_pin_their_json() {
    round_trip(&label());
    for confidentiality in [
        Confidentiality::Public,
        Confidentiality::Secret,
        Confidentiality::Private([].into()),
    ] {
        round_trip(&confidentiality);
    }
    for source in [
        Source::Web,
        Source::File,
        Source::Clipboard,
        Source::Calendar,
        Source::Contacts,
        Source::Notes,
        Source::Cli,
    ] {
        round_trip(&source);
    }
    assert_eq!(round_trip(&Source::Cli), r#"{"kind":"cli"}"#);
    for role in [
        ModelRole::Planner,
        ModelRole::Reader,
        ModelRole::Reviewer,
        ModelRole::PolicyWriter,
        ModelRole::Cua,
        ModelRole::Consolidator,
    ] {
        round_trip(&role);
    }
    let pinned = Label {
        integrity: Integrity::Trusted,
        confidentiality: Confidentiality::Public,
        classes: [DataClass::Voice].into(),
        sources: [Source::User].into(),
    };
    assert_eq!(
        round_trip(&pinned),
        r#"{"integrity":"trusted","confidentiality":{"kind":"public"},"classes":["voice"],"sources":[{"kind":"user"}]}"#
    );
    assert!(Integrity::Untrusted < Integrity::Trusted);
}

#[test]
fn labelled_map_keeps_the_label() {
    let labelled = Labelled::new("abc".to_owned(), label());
    let mapped = labelled.map(|text| text.len());
    assert_eq!(mapped.value, 3);
    assert_eq!(mapped.label, label());
}

#[test]
fn debug_never_shows_content() {
    let secret = "the lawyer's address is 12 Elm St";
    let labelled = Labelled::new(secret.to_owned(), label());
    assert!(!format!("{labelled:?}").contains("Elm"));
    let quarantined = Quarantined::new(labelled);
    let shown = format!("{quarantined:?}");
    assert!(!shown.contains("Elm"), "{shown}");
    assert!(shown.contains(&secret.len().to_string()), "{shown}");
    assert!(shown.contains("Mail"), "sources are shown: {shown}");
}

#[test]
fn quarantined_opens_only_with_the_reader_key() {
    let quarantined = Quarantined::new(Labelled::new("x".to_owned(), label()));
    assert_eq!(quarantined.label(), &label());
    let opened = quarantined.open(&ReaderKey::for_reader_host());
    assert_eq!(opened.value, "x");
}

#[test]
fn entities_and_witnesses_round_trip() {
    let entity = EntityId {
        app: app("org.quire.Mail"),
        kind: EntityKind::parse("mail.thread").expect("kind"),
        key: EntityKey::parse("INBOX/42").expect("key"),
    };
    assert_eq!(
        round_trip(&entity),
        r#"{"app":"org.quire.Mail","kind":"mail.thread","key":"INBOX/42"}"#
    );
    round_trip(&ActionName::parse("mail.thread.archive").expect("action"));
    for input in [
        InputProof::HardwareSeat,
        InputProof::SheetFallback,
        InputProof::ShellCaller,
    ] {
        let receipt = ConfirmReceipt {
            id: ConfirmId::parse("c-1").expect("confirm id"),
            input,
            at: UnixSeconds(1_790_000_000),
        };
        round_trip(&Witness::UserConfirmed(receipt));
    }
}

#[test]
fn malformed_ids_do_not_deserialize() {
    assert!(serde_json::from_str::<EntityKind>("\"Mail\"").is_err());
    assert!(serde_json::from_str::<SessionId>("\"Has Space\"").is_err());
    assert!(serde_json::from_str::<ConfirmId>("\"\"").is_err());
}

#[test]
fn space_vocabulary_is_re_exported_from_porter_core() {
    let scope = SpaceScope::Only(space("work"));
    assert_eq!(round_trip(&scope), r#"{"kind":"only","v":"work"}"#);
}

#[test]
fn slugs_equal_the_serde_forms() {
    use prov::{ActorKind, Effect};
    for kind in [
        ActorKind::User,
        ActorKind::Companion,
        ActorKind::Cua,
        ActorKind::Mcp,
        ActorKind::App,
        ActorKind::ThirdParty,
        ActorKind::System,
        ActorKind::Unknown,
    ] {
        assert_eq!(
            serde_json::to_string(&kind).unwrap(),
            format!("\"{}\"", kind.slug())
        );
    }
    for effect in [
        Effect::Read,
        Effect::UndoableWrite,
        Effect::Outbound,
        Effect::Destructive,
    ] {
        assert_eq!(
            serde_json::to_string(&effect).unwrap(),
            format!("\"{}\"", effect.slug())
        );
    }
}

#[test]
fn trace_attribute_names_are_unique_and_namespaced() {
    use prov::trace::attr;
    let mut seen = std::collections::BTreeSet::new();
    for key in attr::ALL {
        assert!(seen.insert(*key), "{key} is listed twice");
        assert!(key.starts_with("quire."), "{key}");
    }
    assert_eq!(attr::ALL.len(), 13);
    assert_eq!(attr::ACTOR_KIND, "quire.actor.kind");
    assert_eq!(attr::SPACE_HASH, "quire.space.hash");
}

#[test]
fn trace_attributes_carry_no_content_names() {
    for key in prov::trace::attr::ALL {
        for forbidden in [
            "prompt",
            "message",
            "argument",
            "result",
            "title",
            "text",
            "recipient",
        ] {
            assert!(!key.contains(forbidden), "{key}");
        }
    }
}
