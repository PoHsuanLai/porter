//! inferd hosted: the real `Inference1` object on a private bus, a client opening sessions through
//! `porter-client`, and fake OpenAI-compatible engines on Unix sockets behind the real router,
//! supervisor driver, runner and audit sink. Nothing here starts an engine or touches the GPU,
//! the real bus or the user's files.

mod hosting;

use hosting::engine::{Chat, Script};
use hosting::entries;
use hosting::rig::{Plan, World, test_app};
use inferd::catalog::EmbedSpec;
use inferd::peers::Role;
use porter_client::{Accounts, ClientError, DbusTransport, InferSession, TransportError};
use porter_core::capability::{CuaEnv, LlmFeature, Modality};
use porter_core::consent::Usage;
use porter_core::need::{CuaNeed, DimsNeed, EmbedNeed, LlmNeed};
use porter_core::{DataClass, Dims, Need, Tier, Tokens};
use porter_infer::{
    ChatControl, ChatMessage, ChatRequest, ClientFrame, EmbedRequest, EmbedRole, InferEvent,
    InferReply, InferRequest, Knob, MessagePart, Reasoning, Readiness, ReplyShape, Role as ChatRole,
    StopReason, ToolChoice, ToolParallelism,
};
use serde_json::Value;

fn llm() -> Need {
    Need::Llm(LlmNeed {
        features: [LlmFeature::Chat].into(),
        context: Tokens(1000),
    })
}

fn chat_request(text: &str, class: DataClass) -> InferRequest {
    InferRequest::Chat(ChatRequest {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            parts: vec![MessagePart::Text(text.into())],
        }],
        shape: ReplyShape::Text,
        tier: Tier::Balanced,
        class,
        usage: Usage::Interactive,
        tools: vec![],
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

/// Every event up to and including `Finished`.
async fn until_finished(session: &mut impl InferSession) -> Vec<InferEvent> {
    let mut events = Vec::new();
    loop {
        let event = session.next().await.expect("an event");
        let done = matches!(event, InferEvent::Finished(_));
        events.push(event);
        if done {
            return events;
        }
    }
}

fn chat_world(script: Script) -> Plan {
    Plan {
        catalog: vec![("tiny-chat.toml", entries::chat())],
        scripts: vec![("tiny-chat", script)],
        ..Plan::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_porter_client_chat_turn_streams_text_from_the_engine() {
    let world = World::start(chat_world(Script {
        chat: vec![Chat::Say(vec!["Hel", "lo"])],
        dims: 0,
    }))
    .await;
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Notes, Tier::Balanced)
        .await
        .expect("open");
    session
        .send(ClientFrame::Request(chat_request("hi there", DataClass::Notes)))
        .await
        .expect("send");
    let events = until_finished(&mut session).await;
    // The engine was stopped: the client is told it waits, then who answers.
    assert_eq!(events[0], InferEvent::Waiting(Readiness::Loadable));
    let InferEvent::Routed(served) = &events[1] else {
        panic!("Routed after Waiting, got {events:?}");
    };
    assert_eq!(served.model.as_str(), "tiny-chat");
    assert_eq!(served.account.as_str(), "local");
    assert_eq!(
        events[2..4],
        [
            InferEvent::TextDelta("Hel".into()),
            InferEvent::TextDelta("lo".into())
        ]
    );
    let InferEvent::Finished(InferReply::Chat(reply)) = events.last().expect("last") else {
        panic!("a chat reply, got {events:?}");
    };
    assert_eq!(reply.text, "Hello");
    assert_eq!(reply.stop, StopReason::EndTurn);
    assert_eq!((reply.usage.input, reply.usage.output), (Tokens(5), Tokens(2)));

    // What the engine was sent: its served model name and the person's words.
    let bodies = world.engines["tiny-chat"].bodies("/v1/chat/completions");
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["model"], "tiny-chat");
    assert!(bodies[0].to_string().contains("hi there"));

    // The engine was asked for, once, by the first session; and the turn was audited.
    assert_eq!(world.host.spawned.lock().expect("lock").len(), 1);
    let entries = world.audit.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].app, test_app());
    assert_eq!(entries[0].model.as_str(), "tiny-chat");
    assert_eq!(entries[0].usage.input, Tokens(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_one_call_form_the_consumers_use_returns_the_reply() {
    let world = World::start(chat_world(Script {
        chat: vec![Chat::Say(vec!["ok"])],
        dims: 0,
    }))
    .await;
    let reply = world
        .accounts
        .infer(&llm(), DataClass::Notes, Tier::Fast, chat_request("ping", DataClass::Notes))
        .await
        .expect("a reply");
    let InferReply::Chat(chat) = reply else {
        panic!("chat");
    };
    assert_eq!(chat.text, "ok");
}

fn embed_world() -> Plan {
    Plan {
        catalog: vec![("tiny-embed.toml", entries::embed())],
        scripts: vec![(
            "tiny-embed",
            Script {
                chat: vec![],
                dims: 4,
            },
        )],
        embeds: vec![EmbedSpec {
            model: "tiny-embed".into(),
            dims: 4,
            max_input: 512,
            max_batch: 2,
            query_prefix: "search_query: ".into(),
            document_prefix: "search_document: ".into(),
        }],
        ..Plan::default()
    }
}

fn embed_need() -> Need {
    Need::Embeddings(EmbedNeed {
        dims: DimsNeed::Exactly(Dims(4)),
        modalities: [Modality::Text].into(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn an_embedding_request_returns_a_vector_per_text_in_order_with_the_models_prefix() {
    let world = World::start(embed_world()).await;
    let texts: Vec<String> = ["alpha", "beta", "gamma"].map(String::from).to_vec();
    let reply = world
        .accounts
        .infer(
            &embed_need(),
            DataClass::Notes,
            Tier::Fast,
            InferRequest::Embed(EmbedRequest {
                inputs: texts.clone(),
                role: EmbedRole::Document,
                dims: DimsNeed::Exactly(Dims(4)),
                class: DataClass::Notes,
                usage: Usage::Background,
            }),
        )
        .await
        .expect("a reply");
    let InferReply::Embed(embed) = reply else {
        panic!("embed");
    };
    assert_eq!(embed.vectors.len(), 3);
    assert!(embed.vectors.iter().all(|v| v.0.len() == 4));
    assert_ne!(embed.vectors[0], embed.vectors[1], "the order and the texts survive");

    // Three texts under a batch limit of two are two requests, each text carrying the prefix.
    let bodies = world.engines["tiny-embed"].bodies("/v1/embeddings");
    assert_eq!(bodies.len(), 2);
    let inputs = |body: &Value| body["input"].as_array().expect("inputs").len();
    assert_eq!((inputs(&bodies[0]), inputs(&bodies[1])), (2, 1));
    assert_eq!(bodies[0]["input"][0], "search_document: alpha");
    assert_eq!(bodies[1]["input"][0], "search_document: gamma");
}

#[tokio::test(flavor = "multi_thread")]
async fn inferd_absent_is_unreachable() {
    let bus = hosting::bus::PrivateBus::start();
    let accounts = Accounts::over(DbusTransport::over(bus.connect().await));
    let got = accounts
        .session(&llm(), DataClass::Notes, Tier::Fast)
        .await;
    assert!(
        matches!(got, Err(ClientError::Transport(TransportError::Unreachable))),
        "no daemon owns the name: {:?}",
        got.err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_the_table_does_not_name_is_refused_by_the_bus_error() {
    let world = World::start(chat_world(Script::default())).await;
    // A second connection nobody introduced.
    let stranger = Accounts::over(DbusTransport::over(world.bus.connect().await));
    let got = stranger
        .session(&llm(), DataClass::Notes, Tier::Fast)
        .await;
    match got {
        Err(ClientError::Transport(TransportError::Malformed(why))) => {
            assert!(why.contains("caller"), "{why}");
        }
        other => panic!("refused with the daemon's text, got {:?}", other.err()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_class_request_to_a_cloud_only_route_is_refused_by_the_floor() {
    use porter_core::capability::{Capability, LlmCap, LlmWire};
    use porter_core::{AccountId, Billing, Locality, ModelId};
    let cloud = porter_infer::ModelCard {
        account: AccountId::parse("anthropic").expect("id"),
        model: ModelId::parse("sonnet").expect("id"),
        locality: Locality::Cloud { region: None },
        billing: Billing::PlanBudget,
        capabilities: vec![Capability::Llm(LlmCap {
            features: [LlmFeature::Chat].into(),
            context: Tokens(100_000),
            max_output: Tokens(4096),
            wire: LlmWire::Messages,
        })],
    };
    let mut policy = porter_infer::Policy::proposed();
    // The user allows cloud accounts in general; the class's floor is what refuses.
    policy.local_only = porter_infer::LocalOnly::Off;
    let world = World::start(Plan {
        remote: vec![cloud],
        policy,
        ..Plan::default()
    })
    .await;
    let mut session = world
        .accounts
        .session(&llm(), DataClass::Prompt, Tier::Best)
        .await
        .expect("open");
    let events = until_finished(&mut session).await;
    assert_eq!(
        events,
        vec![InferEvent::Finished(InferReply::Refused(
            porter_infer::InferRefusal::RequiresCloud(DataClass::Prompt)
        ))]
    );
    assert!(world.audit.entries().is_empty(), "nothing ran");
    assert!(world.host.spawned.lock().expect("lock").is_empty());
}

fn cua_world(role: Role) -> Plan {
    Plan {
        catalog: vec![("tiny-cua.toml", entries::cua())],
        scripts: vec![(
            "tiny-cua",
            Script {
                chat: vec![Chat::Call {
                    name: "computer_use",
                    arguments: r#"{"action":"left_click","coordinate":[500,500]}"#.into(),
                }],
                dims: 0,
            },
        )],
        role,
        ..Plan::default()
    }
}

fn cua_need() -> Need {
    Need::ComputerUse(CuaNeed {
        environments: [CuaEnv::Desktop].into(),
    })
}

#[path = "hosted/cua.rs"]
mod cua;
