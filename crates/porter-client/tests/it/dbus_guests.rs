//! The guest and computer calls of `Accounts` over the session bus, against a scripted
//! `org.quire.Inference1` on a private bus: each call's decode into typed rows, the answer as
//! the slug the wire carries, a refusal read back as a typed reason, and the stream that hears
//! of a computer asking and of the answers changing.
#![cfg(feature = "dbus")]

use crate::common;

use common::bus::PrivateBus;
use porter_client::lending::{Approval, CandidateModel, GuestAsk, RowState};
use porter_client::{
    Accounts, ClientError, ComputerReason, DbusTransport, GuestAnswer, GuestChange, GuestRow,
    NodeId, TransportError,
};
use porter_core::UnixSeconds;
use porter_dbus::{BusStream, Details};
use std::sync::{Arc, Mutex};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

/// One model of an added computer as the daemon saw it: its id and the (key, shown value) pairs.
type SeenModel = (String, Vec<(String, String)>);

/// What the scripted inferd was asked.
#[derive(Debug, Default)]
struct Asked {
    answers: Vec<(String, String)>,
    forgets: Vec<String>,
    added_nodes: Vec<String>,
    added: Vec<(String, Vec<SeenModel>)>,
    removed: Vec<String>,
}

/// What the scripted inferd answers.
#[derive(Debug, Default, Clone)]
struct Script {
    guests: Vec<(String, Details)>,
    candidates: Vec<(String, Details)>,
    /// A refusal for `AnswerGuest`: the error's name after the prefix, and the sentence.
    refuse_answer: Option<(&'static str, &'static str)>,
    /// A caller the daemon does not let ask: the bus's `AccessDenied` with this text.
    denied: Option<&'static str>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.quire.Inference1.Error.Computer")]
enum Refused {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotAsking(String),
    AlreadyThere(String),
    Unheard(String),
}

#[derive(Debug)]
struct FakeInferd {
    asked: Arc<Mutex<Asked>>,
    script: Script,
}

fn text(value: &str) -> OwnedValue {
    OwnedValue::try_from(Value::from(value.to_owned())).expect("text")
}

fn details(entries: Vec<(&str, OwnedValue)>) -> Details {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn guest_row(node: &str, name: &str, state: &str, since: i64) -> (String, Details) {
    (
        node.to_owned(),
        details(vec![
            ("name", text(name)),
            ("state", text(state)),
            (
                "since",
                OwnedValue::try_from(Value::I64(since)).expect("since"),
            ),
        ]),
    )
}

fn candidate_row(node: &str, name: &str, needs_approval: bool) -> (String, Details) {
    (
        node.to_owned(),
        details(vec![
            ("name", text(name)),
            (
                "models",
                OwnedValue::try_from(Value::new(vec![(
                    "qwen3-8b".to_owned(),
                    "Qwen3 8B".to_owned(),
                )]))
                .expect("models"),
            ),
            (
                "needs_approval",
                OwnedValue::try_from(Value::Bool(needs_approval)).expect("bool"),
            ),
        ]),
    )
}

#[zbus::interface(name = "org.quire.Inference1")]
impl FakeInferd {
    async fn guests(&self) -> fdo::Result<Vec<(String, Details)>> {
        match self.script.denied {
            Some(why) => Err(fdo::Error::AccessDenied(why.to_owned())),
            None => Ok(self.script.guests.clone()),
        }
    }

    async fn answer_guest(&self, node: String, answer: String) -> Result<(), Refused> {
        if let Some((name, words)) = self.script.refuse_answer {
            let words = words.to_owned();
            return Err(match name {
                "NotAsking" => Refused::NotAsking(words),
                "AlreadyThere" => Refused::AlreadyThere(words),
                _ => Refused::Unheard(words),
            });
        }
        self.asked
            .lock()
            .expect("lock")
            .answers
            .push((node, answer));
        Ok(())
    }

    async fn forget_guest(&self, node: String) {
        self.asked.lock().expect("lock").forgets.push(node);
    }

    async fn candidates(&self) -> Vec<(String, Details)> {
        self.script.candidates.clone()
    }

    async fn add_tailnet_computer(&self, node: String) -> String {
        self.asked.lock().expect("lock").added_nodes.push(node);
        "computer:studio".to_owned()
    }

    async fn add_computer(&self, name: String, models: Vec<(String, Details)>) -> String {
        let models = models
            .into_iter()
            .map(|(id, details)| {
                let mut entries: Vec<(String, String)> = details
                    .into_iter()
                    .map(|(key, value)| {
                        let shown = match key.as_str() {
                            "port" => u16::try_from(&value).map(|p| p.to_string()).expect("port"),
                            _ => String::try_from(value).expect("text"),
                        };
                        (key, shown)
                    })
                    .collect();
                entries.sort();
                (id, entries)
            })
            .collect();
        let place = format!("computer:{}", name.to_lowercase());
        self.asked.lock().expect("lock").added.push((name, models));
        place
    }

    async fn remove_computer(&self, name: String) {
        self.asked.lock().expect("lock").removed.push(name);
    }

    #[zbus(signal)]
    async fn guest_asks(
        emitter: &SignalEmitter<'_>,
        node: &str,
        details: Details,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn guests_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct World {
    _bus: PrivateBus,
    daemon: zbus::Connection,
    asked: Arc<Mutex<Asked>>,
    accounts: Accounts<DbusTransport>,
}

impl World {
    async fn start(script: Script) -> Self {
        let bus = PrivateBus::start();
        let daemon = bus.connect().await;
        let asked = Arc::new(Mutex::new(Asked::default()));
        daemon
            .object_server()
            .at(
                porter_dbus::INFERENCE_PATH,
                FakeInferd {
                    asked: Arc::clone(&asked),
                    script,
                },
            )
            .await
            .expect("serve");
        daemon
            .request_name(porter_dbus::INFERENCE_BUS)
            .await
            .expect("own the name");
        let accounts = Accounts::over(DbusTransport::over(bus.connect().await));
        Self {
            _bus: bus,
            daemon,
            asked,
            accounts,
        }
    }

    async fn emitter(&self) -> zbus::object_server::InterfaceRef<FakeInferd> {
        self.daemon
            .object_server()
            .interface::<_, FakeInferd>(porter_dbus::INFERENCE_PATH)
            .await
            .expect("interface")
    }
}

fn node(id: &str) -> NodeId {
    NodeId::parse(id).expect("node id")
}

#[tokio::test(flavor = "multi_thread")]
async fn guests_decode_into_typed_rows() {
    let world = World::start(Script {
        guests: vec![
            guest_row("nPI", "pi", "asking", 1_700_000_000),
            guest_row("nOLD", "old-laptop", "approved", 1_600_000_000),
            guest_row("nBOX", "box", "denied", 5),
        ],
        ..Script::default()
    })
    .await;
    assert_eq!(
        world.accounts.guests().await.expect("guests"),
        vec![
            GuestRow {
                node: node("nPI"),
                name: "pi".to_owned(),
                state: RowState::Asking,
                since: UnixSeconds(1_700_000_000),
            },
            GuestRow {
                node: node("nOLD"),
                name: "old-laptop".to_owned(),
                state: RowState::Approved,
                since: UnixSeconds(1_600_000_000),
            },
            GuestRow {
                node: node("nBOX"),
                name: "box".to_owned(),
                state: RowState::Denied,
                since: UnixSeconds(5),
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_row_that_says_less_than_promised_is_malformed_not_a_panic() {
    for bad in [
        guest_row("nPI", "pi", "sulking", 1),
        ("not a node".to_owned(), guest_row("x", "pi", "asking", 1).1),
        ("nPI".to_owned(), details(vec![("name", text("pi"))])),
    ] {
        let world = World::start(Script {
            guests: vec![bad],
            ..Script::default()
        })
        .await;
        assert!(
            matches!(
                world.accounts.guests().await,
                Err(ClientError::Transport(TransportError::Malformed(_)))
            ),
            "{:?}",
            world.accounts.guests().await
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_answer_crosses_as_its_slug_and_forgetting_names_the_node() {
    let world = World::start(Script::default()).await;
    world
        .accounts
        .answer_guest(&node("nPI"), GuestAnswer::Allow)
        .await
        .expect("allow");
    world
        .accounts
        .answer_guest(&node("nOLD"), GuestAnswer::Deny)
        .await
        .expect("deny");
    world
        .accounts
        .forget_guest(&node("nPI"))
        .await
        .expect("forget");
    let asked = world.asked.lock().expect("lock");
    assert_eq!(
        asked.answers,
        [
            ("nPI".to_owned(), "allow".to_owned()),
            ("nOLD".to_owned(), "deny".to_owned())
        ]
    );
    assert_eq!(asked.forgets, ["nPI"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_comes_back_as_a_typed_reason_with_the_sentence() {
    let world = World::start(Script {
        refuse_answer: Some((
            "NotAsking",
            "That computer is not asking to use this computer's models.",
        )),
        ..Script::default()
    })
    .await;
    assert_eq!(
        world
            .accounts
            .answer_guest(&node("nPI"), GuestAnswer::Allow)
            .await,
        Err(ClientError::Transport(TransportError::Computer {
            reason: ComputerReason::NotAsking,
            words: "That computer is not asking to use this computer's models.".to_owned(),
        }))
    );
    let unknown = World::start(Script {
        refuse_answer: Some(("Unheard", "Something new.")),
        ..Script::default()
    })
    .await;
    assert_eq!(
        unknown
            .accounts
            .answer_guest(&node("nPI"), GuestAnswer::Deny)
            .await,
        Err(ClientError::Transport(TransportError::Computer {
            reason: ComputerReason::Other("Unheard".to_owned()),
            words: "Something new.".to_owned(),
        }))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn candidates_decode_with_their_models_and_approval() {
    let world = World::start(Script {
        candidates: vec![
            candidate_row("nPI", "pi", true),
            candidate_row("nBOX", "box", false),
        ],
        ..Script::default()
    })
    .await;
    let rows = world.accounts.candidates().await.expect("candidates");
    let seen: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.node.as_str().to_owned(),
                row.name.clone(),
                row.models.clone(),
                row.approval,
            )
        })
        .collect();
    let models = vec![CandidateModel::new(
        "qwen3-8b".to_owned(),
        "Qwen3 8B".to_owned(),
    )];
    assert_eq!(
        seen,
        [
            (
                "nPI".to_owned(),
                "pi".to_owned(),
                models.clone(),
                Approval::Needed
            ),
            ("nBOX".to_owned(), "box".to_owned(), models, Approval::Given),
        ]
    );
}

#[cfg(feature = "infer")]
mod computers {
    use super::*;
    use porter_client::{ComputerName, ComputerReach, NewComputer, NewComputerModel};
    use porter_core::{ModelId, SecretText};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tailnet_computer_is_added_by_node_and_answers_its_place() {
        let world = World::start(Script::default()).await;
        let place = world
            .accounts
            .add_tailnet_computer(&node("nPI"))
            .await
            .expect("added");
        assert_eq!(place.as_str(), "computer:studio");
        assert_eq!(world.asked.lock().expect("lock").added_nodes, ["nPI"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_computer_added_by_hand_sends_how_to_reach_each_model_and_is_removed_by_name() {
        let world = World::start(Script::default()).await;
        let computer = NewComputer::new(
            "Studio".to_owned(),
            vec![
                NewComputerModel::new(
                    ModelId::parse("qwen3-8b").expect("id"),
                    ComputerReach::Port(8080),
                ),
                NewComputerModel::new(
                    ModelId::parse("gemma3").expect("id"),
                    ComputerReach::Socket("/run/user/1000/engine.sock".into()),
                )
                .with_key(SecretText::new("sk-1")),
            ],
        );
        let place = world.accounts.add_computer(&computer).await.expect("added");
        assert_eq!(place.as_str(), "computer:studio");
        world
            .accounts
            .remove_computer(&ComputerName::parse("studio").expect("name"))
            .await
            .expect("removed");
        let asked = world.asked.lock().expect("lock");
        assert_eq!(
            asked.added,
            [(
                "Studio".to_owned(),
                vec![
                    (
                        "qwen3-8b".to_owned(),
                        vec![("port".to_owned(), "8080".to_owned())]
                    ),
                    (
                        "gemma3".to_owned(),
                        vec![
                            ("key".to_owned(), "sk-1".to_owned()),
                            ("socket".to_owned(), "/run/user/1000/engine.sock".to_owned())
                        ]
                    ),
                ]
            )]
        );
        assert_eq!(asked.removed, ["studio"]);
    }
}

/// Waits as long as a starved machine needs for a signal that should come.
async fn next(changes: &mut porter_client::GuestChanges) -> Option<GuestChange> {
    tokio::time::timeout(
        porter_fake::GENEROUS,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut *changes).poll_next(cx)),
    )
    .await
    .expect("told in time")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_stream_hears_of_a_computer_asking_and_of_the_answers_changing() {
    let world = World::start(Script::default()).await;
    let mut changes = world.accounts.watch_guests().await.expect("watch");
    let emitter = world.emitter().await;

    let ask = details(vec![
        ("name", text("pi")),
        (
            "since",
            OwnedValue::try_from(Value::I64(42)).expect("since"),
        ),
    ]);
    FakeInferd::guest_asks(emitter.signal_emitter(), "nPI", ask)
        .await
        .expect("signal");
    assert_eq!(
        next(&mut changes).await,
        Some(GuestChange::Asks {
            node: node("nPI"),
            ask: GuestAsk::new("pi".to_owned(), UnixSeconds(42)),
        })
    );

    FakeInferd::guests_changed(emitter.signal_emitter())
        .await
        .expect("signal");
    assert_eq!(next(&mut changes).await, Some(GuestChange::Changed));

    // A question that does not say its computer plainly makes the listener read the list.
    FakeInferd::guest_asks(emitter.signal_emitter(), "nPI", Details::new())
        .await
        .expect("signal");
    assert_eq!(next(&mut changes).await, Some(GuestChange::Changed));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_the_daemon_refuses_is_denied_and_no_daemon_is_unreachable() {
    // The scripted daemon answers as inferd answers a caller that is not Settings or the shell.
    let world = World::start(Script {
        denied: Some("only Settings and the shell may ask"),
        ..Script::default()
    })
    .await;
    assert!(matches!(
        world.accounts.guests().await,
        Err(ClientError::Transport(TransportError::Denied(why))) if why.contains("only Settings")
    ));
    // Nobody owns the name on this bus.
    let bus = PrivateBus::start();
    let alone = Accounts::over(DbusTransport::over(bus.connect().await));
    assert_eq!(
        alone.guests().await,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
    assert_eq!(
        alone.candidates().await,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
}
