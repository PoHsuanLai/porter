//! inferd with a replay engine: the real `Inference1` object on a private bus, a client opening
//! a chat session through `porter-client`, and an engine that plays a cassette instead of
//! running a program. The cassette is the docket acceptance's flow (a): the policy writer's draft
//! and the planner's four steps.

mod hosting;

use hosting::rig::{Plan, World};
use inferd::replay::cassette::Cassette;
use porter_client::InferSession;
use porter_core::capability::LlmFeature;
use porter_core::consent::Usage;
use porter_core::need::LlmNeed;
use porter_core::{DataClass, Need, Tier, Tokens};
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, InferEvent, InferReply, InferRequest,
    JsonSchemaText, JsonText, Knob, MessagePart, Reasoning, ReplyShape, Role as ChatRole,
    StopReason, ToolChoice, ToolDecl, ToolName, ToolParallelism,
};
use std::path::PathBuf;

fn cassette_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cassettes/docket-flow-a.jsonl")
}

fn planner_need() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat, LlmFeature::Tools].into(),
        context: Tokens(1000),
    })
}

fn request(text: &str, tools: Vec<ToolDecl>) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            parts: vec![MessagePart::Text(text.into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class: DataClass::Mail,
        usage: Usage::Interactive,
        tools,
        control: ChatControl {
            tool_choice: ToolChoice::Auto,
            tool_calls: ToolParallelism::One,
            max_output: Knob::Off,
            reasoning: Reasoning::EngineDefault,
            sampling: Knob::Off,
            stop: vec![],
        },
    })
}

fn tool() -> ToolDecl {
    ToolDecl {
        name: ToolName::parse("org.quire.Mail-mail.thread.search").expect("name"),
        description: "find threads".into(),
        params: JsonSchemaText(JsonText::parse(r#"{"type":"object"}"#).expect("json")),
    }
}

async fn finished(session: &mut impl InferSession, asked: InferRequest) -> InferReply {
    // An engine that cannot start now fails the session at Open, before the request is sent: a
    // closed session is not an error here, its `Finished` is still to be read.
    let _ = session.send(ClientFrame::Request(asked)).await;
    loop {
        if let InferEvent::Finished(reply) = session.next().await.expect("an event") {
            return reply;
        }
    }
}

fn replay_world(file: PathBuf) -> Plan {
    Plan {
        replay: vec![("scripted", file)],
        ..Plan::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_session_is_answered_from_the_cassette() {
    let world = World::start(replay_world(cassette_file())).await;
    let mut session = world
        .accounts
        .session(&planner_need(), DataClass::Mail, Tier::Balanced)
        .await
        .expect("open");
    // The planner asks with tools: its first step is a tool call the way an engine makes one.
    let InferReply::Chat(first) = finished(&mut session, request("forward it", vec![tool()])).await
    else {
        panic!("a chat reply");
    };
    assert_eq!(first.stop, StopReason::ToolUse);
    assert_eq!(first.served.model.as_str(), "scripted");
    assert_eq!(first.tool_calls.len(), 1);
    assert_eq!(
        first.tool_calls[0].name.as_str(),
        "org.quire.Mail-mail.thread.search"
    );
    assert_eq!(first.tool_calls[0].args.as_str(), r#"{"query":"Lisbon"}"#);
    // Entries are played in order: the next planner request gets the next step.
    let InferReply::Chat(second) = finished(&mut session, request("go on", vec![tool()])).await
    else {
        panic!("a chat reply");
    };
    assert_eq!(
        second.tool_calls[0].name.as_str(),
        "org.quire.Mail-mail.contact.search"
    );
    // Nothing was spawned: the engine is the daemon's own task.
    assert_eq!(
        world.host.spawned.lock().expect("lock").len(),
        0,
        "replay engines are not processes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_no_entry_answers_fails_as_a_typed_error() {
    let world = World::start(replay_world(cassette_file())).await;
    let mut session = world
        .accounts
        .session(&planner_need(), DataClass::Mail, Tier::Balanced)
        .await
        .expect("open");
    // Without tools and without the writer's words: nothing in the cassette admits it.
    let reply = finished(&mut session, request("anything", vec![])).await;
    assert!(
        matches!(reply, InferReply::Failed(_)),
        "a failed reply, not a chat one: {reply:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_cassette_leaves_the_engine_unable_to_start() {
    let world = World::start(replay_world(PathBuf::from("/nonexistent/cassette.jsonl"))).await;
    let mut session = world
        .accounts
        .session(&planner_need(), DataClass::Mail, Tier::Balanced)
        .await
        .expect("open");
    let reply = finished(&mut session, request("hi", vec![tool()])).await;
    assert!(!matches!(reply, InferReply::Chat(_)), "{reply:?}");
}

#[test]
fn the_shipped_cassette_reads() {
    let text = std::fs::read_to_string(cassette_file()).expect("file");
    let cassette = Cassette::parse(&text).expect("cassette");
    assert_eq!(cassette.entries.len(), 5);
}
