//! `EngineHost`: inference with no inferd, against porter's fake OpenAI-compatible server on
//! loopback (no network, no real model).
#![cfg(feature = "engines")]

use porter_client::engines::{
    Dialect, Engine, EngineHost, EngineId, EngineUrl, KeySource, NoKeys, Route,
};
use porter_client::{InferRefusal, InferSession, OpenOptions, Policy, SessionHost, Slot};
use porter_core::need::{DimsNeed, EmbedNeed, LlmNeed};
use porter_core::{
    AccountId, AppId, AppName, DataClass, Isolation, Locality, Need, SecretText, Tier, Tokens,
};
use porter_fake_servers::{FakeModels, ModelDef, ModelsHandle, Running, Wire};
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferReply, InferRequest, Knob,
    MessagePart, ModelError, Reasoning, ReplyShape, Role, ToolChoice, ToolParallelism,
};
use std::sync::{Arc, Mutex};

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.example.Inapp").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn need() -> Need {
    Need::Llm(LlmNeed {
        features: Default::default(),
        context: Tokens(1),
    })
}

fn chat(class: DataClass) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts: vec![MessagePart::Text("hello".into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class,
        usage: porter_core::consent::Usage::Interactive,
        tools: vec![],
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::Many,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
        },
    })
}

fn id(text: &str) -> EngineId {
    EngineId::parse(text).expect("id")
}

fn engine(name: &str, base: &str, locality: Locality) -> Engine {
    Engine::new(
        id(name),
        AccountId::parse(name).expect("account"),
        "llama3.2:3b",
        EngineUrl::parse(&format!("{base}/v1")).expect("url"),
        locality,
        Dialect::LlamaServer,
        Tokens(256),
    )
    .expect("engine")
}

fn cloud() -> Locality {
    Locality::Cloud { region: None }
}

fn row(engines: &[&str]) -> Route {
    Route {
        slot: Slot::Text,
        tier: Tier::Balanced,
        engines: engines.iter().map(|name| id(name)).collect(),
    }
}

/// Every class may go anywhere unless a floor says otherwise: what the app chose to say.
fn open_policy() -> Policy {
    Policy {
        local_only: porter_infer::LocalOnly::Off,
        floors: vec![],
    }
}

async fn fake(key: Option<&str>) -> Running<ModelsHandle> {
    FakeModels::start(Wire::OpenAi, vec![ModelDef::chat("llama3.2:3b", 8192)], key)
        .await
        .expect("fake")
}

async fn answer<S: InferSession>(session: &mut S, request: InferRequest) -> Vec<InferEvent> {
    session
        .send(ClientFrame::Request(request))
        .await
        .expect("send");
    let mut events = vec![];
    loop {
        let event = session.next().await.expect("event");
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

fn finished(events: &[InferEvent]) -> &InferReply {
    match events.last() {
        Some(InferEvent::Finished(reply)) => reply,
        other => panic!("not finished: {other:?}"),
    }
}

async fn open<K: KeySource + 'static>(
    host: &EngineHost<K>,
    class: DataClass,
) -> porter_client::engines::EngineSession<K> {
    host.open(
        &app(),
        &need(),
        class,
        Tier::Balanced,
        &OpenOptions::default(),
    )
    .await
    .expect("session")
}

#[tokio::test]
async fn a_chat_streams_its_events_from_the_local_engine() {
    let server = fake(None).await;
    server.say(&["Hel", "lo"]);
    let local = engine("ollama", server.base_url(), Locality::OnDevice);
    let host =
        EngineHost::new(vec![local], vec![row(&["ollama"])], open_policy(), NoKeys).expect("host");
    let mut session = open(&host, DataClass::Mail).await;
    let events = answer(&mut session, chat(DataClass::Mail)).await;

    assert!(matches!(&events[0], InferEvent::Routed(served)
        if served.account.as_str() == "ollama"
            && served.model.as_str() == "llama3.2-3b"
            && served.locality == Locality::OnDevice));
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            InferEvent::TextDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hel", "lo"]);
    match finished(&events) {
        InferReply::Chat(reply) => assert_eq!(reply.text, "Hello"),
        other => panic!("{other:?}"),
    }
    // The request carried the served name and the engine's reply limit, and no temperature:
    // the app chose no sampling, so it is the engine's.
    let sent = &server.chats()[0];
    assert_eq!(sent["model"], "llama3.2:3b");
    assert_eq!(sent["max_tokens"], 256);
    assert!(sent.get("temperature").is_none(), "{sent}");
    assert_eq!(server.hits()[0].authorization, None);

    // A second request on the same session is a second turn; Routed is not repeated.
    let again = answer(&mut session, chat(DataClass::Mail)).await;
    assert!(!again.iter().any(|e| matches!(e, InferEvent::Routed(_))));
    assert_eq!(server.chats().len(), 2);
}

#[tokio::test]
async fn a_class_that_stays_on_this_computer_never_reaches_a_cloud_engine() {
    let server = fake(None).await;
    let hosted = engine("hosted", server.base_url(), cloud());
    // Policy::proposed puts Mail on this computer; local-only is off so the cloud engine is
    // seen and then refused by the floor, not dropped.
    let policy = Policy {
        local_only: porter_infer::LocalOnly::Off,
        ..Policy::proposed()
    };
    let host = EngineHost::new(vec![hosted], vec![row(&["hosted"])], policy, NoKeys).expect("host");

    let mut session = open(&host, DataClass::Mail).await;
    let events = answer(&mut session, chat(DataClass::Mail)).await;
    assert_eq!(
        finished(&events),
        &InferReply::Refused(InferRefusal::RequiresCloud(DataClass::Mail))
    );
    assert_eq!(events.len(), 1, "the refusal is the first and only event");
    assert!(server.hits().is_empty(), "nothing was sent");

    // A class with no floor may go there.
    let mut open_class = open(&host, DataClass::Public).await;
    let events = answer(&mut open_class, chat(DataClass::Public)).await;
    assert!(matches!(finished(&events), InferReply::Chat(_)));
}

#[tokio::test]
async fn local_only_drops_cloud_engines_and_the_row_falls_through_to_the_local_one() {
    let hosted_server = fake(None).await;
    let local_server = fake(None).await;
    let hosted = engine("hosted", hosted_server.base_url(), cloud());
    let local = engine("ollama", local_server.base_url(), Locality::OnDevice);
    let table = |policy| {
        EngineHost::new(
            vec![hosted.clone(), local.clone()],
            vec![row(&["hosted", "ollama"])],
            policy,
            NoKeys,
        )
        .expect("host")
    };

    let only_local = table(Policy {
        local_only: porter_infer::LocalOnly::On,
        floors: vec![],
    });
    let mut session = open(&only_local, DataClass::Public).await;
    let events = answer(&mut session, chat(DataClass::Public)).await;
    assert!(matches!(finished(&events), InferReply::Chat(_)));
    assert!(hosted_server.hits().is_empty());
    assert_eq!(local_server.chats().len(), 1);

    // With nothing forbidden, the app's order wins: the first engine of the row.
    let anywhere = table(open_policy());
    let mut session = open(&anywhere, DataClass::Public).await;
    answer(&mut session, chat(DataClass::Public)).await;
    assert_eq!(hosted_server.chats().len(), 1);
}

/// A key source holding one key per account, and counting how often it was asked.
#[derive(Debug, Default)]
struct Keychain {
    keys: Mutex<Vec<(String, String)>>,
}

impl KeySource for Keychain {
    async fn key(&self, account: &AccountId) -> Option<SecretText> {
        self.keys
            .lock()
            .expect("lock")
            .iter()
            .find(|(name, _)| name == account.as_str())
            .map(|(_, key)| SecretText::new(key.clone()))
    }
}

#[tokio::test]
async fn a_missing_key_is_a_typed_refusal_and_a_held_one_goes_as_the_bearer() {
    let server = fake(Some("sk-fake")).await;
    let hosted = engine("hosted", server.base_url(), cloud()).with_key();
    let keychain = Arc::new(Keychain::default());
    let host = EngineHost::new(
        vec![hosted],
        vec![row(&["hosted"])],
        open_policy(),
        Arc::clone(&keychain),
    )
    .expect("host");

    let mut session = open(&host, DataClass::Public).await;
    let events = answer(&mut session, chat(DataClass::Public)).await;
    assert_eq!(
        finished(&events),
        &InferReply::Refused(InferRefusal::NeedsGrant)
    );
    assert!(server.hits().is_empty());

    keychain
        .keys
        .lock()
        .expect("lock")
        .push(("hosted".into(), "sk-fake".into()));
    let mut session = open(&host, DataClass::Public).await;
    let events = answer(&mut session, chat(DataClass::Public)).await;
    assert!(matches!(finished(&events), InferReply::Chat(_)));
    assert_eq!(
        server.hits()[0].authorization.as_deref(),
        Some("Bearer sk-fake")
    );
}

#[tokio::test]
async fn needs_without_a_row_and_kinds_of_need_not_served_are_refused() {
    let server = fake(None).await;
    let local = engine("ollama", server.base_url(), Locality::OnDevice);
    let host =
        EngineHost::new(vec![local], vec![row(&["ollama"])], open_policy(), NoKeys).expect("host");

    // No row for the Fast tier: nothing is chosen by default.
    let mut fast = host
        .open(
            &app(),
            &need(),
            DataClass::Public,
            Tier::Fast,
            &OpenOptions::default(),
        )
        .await
        .expect("session");
    assert_eq!(
        fast.next().await,
        Ok(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unavailable
        )))
    );
    assert_eq!(fast.next().await, Err(porter_client::SessionError::Closed));

    let embeddings = Need::Embeddings(EmbedNeed {
        dims: DimsNeed::Any,
        modalities: Default::default(),
    });
    let mut other = host
        .open(
            &app(),
            &embeddings,
            DataClass::Public,
            Tier::Balanced,
            &OpenOptions::default(),
        )
        .await
        .expect("session");
    assert_eq!(
        other.next().await,
        Ok(InferEvent::Finished(InferReply::Refused(
            InferRefusal::Unsupported
        )))
    );
}

#[tokio::test]
async fn a_request_of_another_class_than_the_session_opened_with_is_refused() {
    let server = fake(None).await;
    let local = engine("ollama", server.base_url(), Locality::OnDevice);
    let host =
        EngineHost::new(vec![local], vec![row(&["ollama"])], open_policy(), NoKeys).expect("host");
    let mut session = open(&host, DataClass::Public).await;
    let events = answer(&mut session, chat(DataClass::Mail)).await;
    assert_eq!(
        finished(&events),
        &InferReply::Refused(InferRefusal::Unsupported)
    );
    assert!(server.hits().is_empty());
}

#[tokio::test]
async fn an_engine_that_is_not_running_is_a_failed_turn_not_a_hang() {
    // A port nobody listens on: bind and drop.
    let free = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = free.local_addr().expect("addr").port();
    drop(free);
    let local = engine(
        "ollama",
        &format!("http://127.0.0.1:{port}"),
        Locality::OnDevice,
    );
    let host =
        EngineHost::new(vec![local], vec![row(&["ollama"])], open_policy(), NoKeys).expect("host");
    let mut session = open(&host, DataClass::Public).await;
    let events = answer(&mut session, chat(DataClass::Public)).await;
    assert_eq!(
        finished(&events),
        &InferReply::Failed(ModelError::Unreachable)
    );
}

#[tokio::test]
async fn prepare_says_what_routing_says() {
    let server = fake(None).await;
    let local = engine("ollama", server.base_url(), Locality::OnDevice);
    let host =
        EngineHost::new(vec![local], vec![row(&["ollama"])], open_policy(), NoKeys).expect("host");
    let options = OpenOptions::default();
    assert_eq!(
        host.prepare(&app(), &need(), DataClass::Public, Tier::Balanced, &options)
            .await,
        Ok(porter_infer::Readiness::Ready)
    );
    assert_eq!(
        host.prepare(&app(), &need(), DataClass::Public, Tier::Best, &options)
            .await,
        Ok(porter_infer::Readiness::Unavailable)
    );
}
