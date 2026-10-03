//! `InProcess` and inference: an app that hosts accounts alone has no inferd (a session is
//! `Unreachable`, as on a bus with no daemon, so a caller degrades); one that hands in a broker
//! gets the broker's sessions, opened for the app the host names.

use porter_client::{
    Accounts, ClientError, InProcess, InferSession, NoBroker, SessionHost, TransportError,
};
use porter_core::need::LlmNeed;
use porter_core::{AppId, AppName, DataClass, Isolation, Need, Tier, Tokens};
use porter_fake::fake_service;
use porter_fake::{FakeInferSession, FakeService, Script, ScriptStep, ScriptedPrompter};
use porter_infer::{
    ChatControl, ChatReply, ClientFrame, InferEvent, InferReply, InferRequest, Knob, OpenOptions,
    Reasoning, ReplyShape, RequestKind, ServedBy, StopReason, TokenUsage, ToolChoice,
    ToolParallelism, Traceparent,
};
use std::sync::{Arc, Mutex};

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Mail").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn need() -> Need {
    Need::Llm(LlmNeed {
        features: Default::default(),
        context: Tokens(1),
    })
}

fn served() -> ServedBy {
    ServedBy {
        account: porter_core::AccountId::parse("local").expect("id"),
        model: porter_core::ModelId::parse("echo").expect("id"),
        locality: porter_core::Locality::OnDevice,
    }
}

fn chat() -> InferRequest {
    InferRequest::Chat(porter_infer::ChatRequest {
        messages: vec![],
        shape: ReplyShape::Text,
        tier: Tier::Fast,
        class: DataClass::Public,
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

fn reply() -> InferReply {
    InferReply::Chat(ChatReply {
        text: "hello".into(),
        tool_calls: vec![],
        stop: StopReason::EndTurn,
        thought: None,
        usage: TokenUsage {
            input: Tokens(1),
            output: Tokens(1),
            cached: Tokens(0),
        },
        served: served(),
    })
}

async fn service() -> Arc<FakeService> {
    Arc::new(fake_service(ScriptedPrompter::answering([])).await)
}

#[tokio::test]
async fn with_no_broker_a_session_is_unreachable_and_accounts_still_answer() {
    let accounts = Accounts::over(InProcess::new(service().await, app()));
    assert!(matches!(
        accounts
            .session(&need(), DataClass::Public, Tier::Fast)
            .await,
        Err(ClientError::Transport(TransportError::Unreachable))
    ));
    assert_eq!(
        accounts
            .infer(&need(), DataClass::Public, Tier::Fast, chat())
            .await,
        Err(ClientError::Transport(TransportError::Unreachable))
    );
    assert_eq!(accounts.grants().await, Ok(vec![]));
    let _ = NoBroker;
}

/// What the host was asked to open.
type Opened = Vec<(AppId, DataClass, Tier, OpenOptions)>;

#[derive(Debug, Default)]
struct Broker {
    opened: Mutex<Opened>,
}

impl SessionHost for Broker {
    type Session = FakeInferSession;

    async fn open(
        &self,
        app: &AppId,
        _need: &Need,
        class: DataClass,
        tier: Tier,
        options: &OpenOptions,
    ) -> Result<FakeInferSession, TransportError> {
        self.opened
            .lock()
            .expect("lock")
            .push((app.clone(), class, tier, options.clone()));
        Ok(FakeInferSession::scripted([Script {
            kind: RequestKind::Chat,
            steps: vec![
                ScriptStep::Emit(InferEvent::Routed(served())),
                ScriptStep::Emit(InferEvent::Finished(reply())),
            ],
        }]))
    }
}

#[tokio::test]
async fn a_hosted_broker_serves_the_sessions_for_the_app_the_host_names() {
    let broker = Arc::new(Broker::default());
    let accounts =
        Accounts::over(InProcess::new(service().await, app()).with_broker(Arc::clone(&broker)));
    let parent = Traceparent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        .expect("traceparent");
    let options = OpenOptions {
        traceparent: Some(parent),
    };
    let mut session = accounts
        .session_with(&need(), DataClass::Public, Tier::Fast, &options)
        .await
        .expect("a session");
    session
        .send(ClientFrame::Request(chat()))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::Routed(served())));
    assert_eq!(session.next().await, Ok(InferEvent::Finished(reply())));
    assert_eq!(
        *broker.opened.lock().expect("lock"),
        vec![(app(), DataClass::Public, Tier::Fast, options)]
    );

    // The one-call form goes through the same door.
    assert_eq!(
        accounts
            .infer(&need(), DataClass::Public, Tier::Fast, chat())
            .await,
        Ok(reply())
    );
}
