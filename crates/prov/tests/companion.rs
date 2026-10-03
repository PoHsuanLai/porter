//! The companion amendment: the one message model, the roster reference, and the desktop join
//! rule. Serde round trips with pinned JSON, and the pure tables.

use prov::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
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

fn space(id: &str) -> SpaceId {
    SpaceId::parse(id).expect("space id")
}

fn worker(task: &str) -> AgentRef {
    AgentRef::Worker {
        task: TaskId::parse(task).expect("task id"),
    }
}

fn at(agent: AgentRef, s: &str) -> Address {
    Address::new(agent, space(s))
}

fn label(integrity: Integrity, confidentiality: Confidentiality, sources: &[Source]) -> Label {
    Label {
        integrity,
        confidentiality,
        classes: BTreeSet::new(),
        sources: sources.iter().cloned().collect(),
    }
}

fn private(spaces: &[&str]) -> Confidentiality {
    Confidentiality::Private(spaces.iter().map(|s| space(s)).collect())
}

fn message(kind: MessageKind, parts: Vec<Part>) -> Message {
    Message {
        id: MessageId::parse("m-2").expect("id"),
        thread: ThreadId::parse("m-1").expect("id"),
        in_reply_to: Some(MessageId::parse("m-1").expect("id")),
        from: at(worker("t-4"), "work"),
        to: at(AgentRef::Companion, "work"),
        kind,
        parts,
        label: label(
            Integrity::Untrusted,
            private(&["work"]),
            &[Source::Mail, Source::Model(ModelRole::Reader)],
        ),
        sent: UnixSeconds(1_700_000_000),
    }
}

fn text(t: &str) -> Part {
    Part::Text(MessageText::new(t))
}

fn entity() -> EntityId {
    EntityId {
        app: AppName::parse("org.quire.Mail").expect("app"),
        kind: EntityKind::parse("mail.thread").expect("kind"),
        key: EntityKey::parse("t/9").expect("key"),
    }
}

#[test]
fn every_agent_ref_address_kind_and_part_round_trips() {
    for agent in [
        AgentRef::Companion,
        worker("t-4"),
        AgentRef::Cua {
            run: RunId::parse("r-7").expect("run"),
        },
        AgentRef::User,
    ] {
        round_trip(&at(agent.clone(), "work"));
        round_trip(&agent);
    }
    assert_eq!(round_trip(&AgentRef::Companion), r#"{"kind":"companion"}"#);
    assert_eq!(
        round_trip(&worker("t-4")),
        r#"{"kind":"worker","v":{"task":"t-4"}}"#
    );
    assert_eq!(
        round_trip(&at(AgentRef::User, "home")),
        r#"{"agent":{"kind":"user"},"space":"home"}"#
    );
    let kinds = [
        (MessageKind::Note, r#"{"kind":"note"}"#.to_owned()),
        (MessageKind::Request, r#"{"kind":"request"}"#.to_owned()),
    ]
    .into_iter()
    .chain(
        [
            (ReportStatus::Done, "done"),
            (ReportStatus::Failed, "failed"),
            (ReportStatus::Cancelled, "cancelled"),
            (ReportStatus::Progress, "progress"),
        ]
        .map(|(status, slug)| {
            (
                MessageKind::Report { status },
                format!(r#"{{"kind":"report","v":{{"status":"{slug}"}}}}"#),
            )
        }),
    );
    for (kind, json) in kinds {
        assert_eq!(round_trip(&kind), json);
    }
    assert_eq!(round_trip(&text("hi")), r#"{"kind":"text","v":"hi"}"#);
    assert_eq!(
        round_trip(&Part::Outcome(OutcomeRef::parse("out:41").expect("ref"))),
        r#"{"kind":"outcome","v":"out:41"}"#
    );
    assert_eq!(
        round_trip(&Part::Undo(UndoHandle::parse("u-3").expect("ref"))),
        r#"{"kind":"undo","v":"u-3"}"#
    );
    round_trip(&Part::Entity(entity()));
}

#[test]
fn a_message_round_trips_and_carries_its_label() {
    let m = message(
        MessageKind::Report {
            status: ReportStatus::Done,
        },
        vec![text("found 3"), Part::Entity(entity())],
    );
    let json = round_trip(&m);
    assert!(json.contains(r#""in_reply_to":"m-1""#), "{json}");
    assert!(json.contains(r#""integrity":"untrusted""#), "{json}");
    assert_eq!(m.text(), "found 3");
    // Debug never shows the words.
    assert!(!format!("{:?}", text("secret words")).contains("secret"));
    // A first message has no reply target.
    let first = Message {
        in_reply_to: None,
        ..m
    };
    assert!(round_trip(&first).contains(r#""in_reply_to":null"#));
}

#[test]
fn a_message_may_cross_spaces_and_keeps_both_ends() {
    let mut m = message(MessageKind::Note, vec![text("fyi")]);
    assert_eq!(m.crossing(), Crossing::Within);
    m.to = at(AgentRef::Companion, "home");
    assert_eq!(m.crossing(), Crossing::Across);
    assert_eq!(m.from.space, space("work"));
    assert_eq!(m.to.space, space("home"));
    assert_eq!(
        at(AgentRef::User, "a").crossing(&at(AgentRef::User, "a")),
        Crossing::Within
    );
}

#[test]
fn check_finds_the_first_fault() {
    let long = "x".repeat(MAX_TEXT_BYTES + 1);
    let many: Vec<Part> = (0..=MAX_PARTS).map(|_| text("a")).collect();
    let mut reply_self = message(MessageKind::Note, vec![text("a")]);
    reply_self.in_reply_to = Some(reply_self.id.clone());
    let mut user_report = message(
        MessageKind::Report {
            status: ReportStatus::Done,
        },
        vec![text("a")],
    );
    user_report.from = at(AgentRef::User, "work");
    let cases: Vec<(Message, Result<(), Fault>)> = vec![
        (message(MessageKind::Request, vec![text("go")]), Ok(())),
        (
            message(MessageKind::Note, vec![Part::Entity(entity())]),
            Ok(()),
        ),
        (message(MessageKind::Note, vec![]), Err(Fault::NoParts)),
        (message(MessageKind::Note, many), Err(Fault::TooManyParts)),
        (
            message(MessageKind::Note, vec![text(&long)]),
            Err(Fault::TextTooLong),
        ),
        (reply_self, Err(Fault::RepliesToItself)),
        (user_report, Err(Fault::ReportFromUser)),
    ];
    for (m, want) in cases {
        assert_eq!(m.check(), want, "{m:?}");
    }
}

#[test]
fn the_roster_reference_follows_the_actor() {
    let session = SessionId::parse("s-1").expect("id");
    let companion = |role| Actor::Companion {
        session: session.clone(),
        role,
    };
    let run = RunId::parse("r-7").expect("run");
    let cases = [
        (
            Actor::User {
                via: AppName::parse("org.quire.Shell").expect("app"),
            },
            Some(AgentRef::User),
        ),
        (companion(AgentRole::Planner), Some(AgentRef::Companion)),
        (companion(AgentRole::Reader), Some(AgentRef::Companion)),
        (
            companion(AgentRole::Worker {
                task: TaskId::parse("t-4").expect("task"),
            }),
            Some(worker("t-4")),
        ),
        (
            companion(AgentRole::Cua { run: run.clone() }),
            Some(AgentRef::Cua { run }),
        ),
        (
            Actor::System {
                part: SystemPart::Router,
            },
            None,
        ),
        (Actor::Unknown, None),
        (Actor::Cli, None),
    ];
    for (actor, want) in cases {
        assert_eq!(AgentRef::of(&actor), want, "{actor:?}");
    }
    let m = message(MessageKind::Note, vec![text("a")]);
    let as_worker = companion(AgentRole::Worker {
        task: TaskId::parse("t-4").expect("task"),
    });
    assert_eq!(m.sender_matches(&as_worker), SenderCheck::Matches);
    assert_eq!(
        m.sender_matches(&companion(AgentRole::Planner)),
        SenderCheck::Mismatch
    );
    assert_eq!(m.sender_matches(&Actor::Unknown), SenderCheck::Mismatch);
}

#[test]
fn confidentiality_join_table() {
    use Confidentiality::{Public, Secret};
    let cases: Vec<(Confidentiality, Confidentiality, Confidentiality)> = vec![
        (Public, Public, Public),
        (Public, private(&["work"]), private(&["work"])),
        (
            private(&["work"]),
            private(&["home"]),
            private(&["home", "work"]),
        ),
        (private(&["work"]), private(&["work"]), private(&["work"])),
        // desktop is a sub-scope of every Space: it vanishes beside a real one
        (
            private(&["desktop"]),
            private(&["work"]),
            private(&["work"]),
        ),
        (
            private(&["desktop"]),
            private(&["desktop"]),
            private(&["desktop"]),
        ),
        (Public, private(&["desktop"]), private(&["desktop"])),
        (
            private(&["desktop"]),
            private(&["home", "work"]),
            private(&["home", "work"]),
        ),
        // a value that was not normalised comes out normalised
        (Public, private(&["desktop", "work"]), private(&["work"])),
        (Secret, Public, Secret),
        (private(&["work"]), Secret, Secret),
        (Secret, Secret, Secret),
    ];
    for (a, b, want) in cases {
        assert_eq!(a.join(&b), want, "{a:?} join {b:?}");
        assert_eq!(b.join(&a), want, "{b:?} join {a:?}");
    }
}

#[test]
fn confidentiality_join_is_a_semilattice() {
    let universe = [
        Confidentiality::Public,
        Confidentiality::Secret,
        private(&["desktop"]),
        private(&["work"]),
        private(&["home"]),
        private(&["home", "work"]),
    ];
    for a in &universe {
        assert_eq!(&a.join(a), a, "idempotent {a:?}");
        for b in &universe {
            assert_eq!(a.join(b), b.join(a), "commutative {a:?} {b:?}");
            for c in &universe {
                assert_eq!(
                    a.join(b).join(c),
                    a.join(&b.join(c)),
                    "associative {a:?} {b:?} {c:?}"
                );
            }
        }
    }
}

#[test]
fn data_flows_only_where_every_named_space_allows() {
    use Flow::{Allowed, Denied};
    let cases = [
        (Confidentiality::Public, "work", Allowed),
        (private(&["work"]), "work", Allowed),
        (private(&["work"]), "home", Denied),
        (private(&["desktop"]), "work", Allowed),
        (private(&["desktop"]), "desktop", Allowed),
        (private(&["home", "work"]), "work", Denied),
        (private(&["desktop", "work"]), "work", Allowed),
        (Confidentiality::Secret, "work", Denied),
    ];
    for (conf, to, want) in cases {
        assert_eq!(conf.flow_to(&space(to)), want, "{conf:?} to {to}");
    }
}

#[test]
fn the_desktop_scope_admits_only_what_the_person_stated() {
    use DesktopVerdict::*;
    let user = &[Source::User];
    let cases = [
        (
            label(Integrity::Trusted, Confidentiality::Public, user),
            Admit,
        ),
        (
            label(Integrity::Trusted, private(&["desktop"]), user),
            Admit,
        ),
        (
            label(Integrity::Trusted, private(&["work"]), user),
            SpaceBound,
        ),
        (
            label(Integrity::Trusted, Confidentiality::Secret, user),
            Secret,
        ),
        (
            label(Integrity::Untrusted, Confidentiality::Public, user),
            Untrusted,
        ),
        (
            label(
                Integrity::Untrusted,
                Confidentiality::Secret,
                &[Source::Mail],
            ),
            Secret,
        ),
        (
            label(
                Integrity::Trusted,
                Confidentiality::Public,
                &[Source::App(AppName::parse("org.quire.Mail").expect("app"))],
            ),
            NotUserStated,
        ),
        (
            label(
                Integrity::Trusted,
                Confidentiality::Public,
                &[Source::User, Source::Model(ModelRole::Consolidator)],
            ),
            NotUserStated,
        ),
        (
            label(Integrity::Trusted, Confidentiality::Public, &[]),
            NotUserStated,
        ),
    ];
    for (l, want) in cases {
        assert_eq!(desktop_admits(&l), want, "{l:?}");
    }
}
