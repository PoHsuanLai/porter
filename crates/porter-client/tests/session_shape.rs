//! `Accounts::session` and `Accounts::infer` over a transport whose sessions are scripted.

use porter_client::{Accounts, ClientError, InferSession, Transport, TransportError};
use porter_core::need::LlmNeed;
use porter_core::{AccountsReply, AccountsRequest, DataClass, Need, Tier, Tokens};
use porter_fake::{FakeInferSession, Script, ScriptStep};
use porter_infer::OpenOptions;
use porter_infer::{ChatControl, Knob, Reasoning, ToolChoice, ToolParallelism};
use porter_infer::{
    ChatReply, ClientFrame, InferEvent, InferRefusal, InferReply, InferRequest, ReplyShape,
    RequestKind, ServedBy, StopReason, TokenUsage,
};
use std::sync::Mutex;

#[derive(Debug)]
struct Scripted(Mutex<Option<FakeInferSession>>);

impl Transport for Scripted {
    type Session = FakeInferSession;

    async fn call(&self, _request: AccountsRequest) -> Result<AccountsReply, TransportError> {
        Err(TransportError::Unreachable)
    }

    async fn open_with(
        &self,
        _need: &Need,
        _class: DataClass,
        _tier: Tier,
        _options: &OpenOptions,
    ) -> Result<FakeInferSession, TransportError> {
        self.0
            .lock()
            .expect("lock")
            .take()
            .ok_or(TransportError::Unreachable)
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

fn accounts(script: Script) -> Accounts<Scripted> {
    Accounts::over(Scripted(Mutex::new(Some(FakeInferSession::scripted([
        script,
    ])))))
}

#[tokio::test]
async fn infer_reads_to_the_finished_event() {
    let usage = TokenUsage {
        input: Tokens(1),
        output: Tokens(1),
        cached: Tokens(0),
    };
    let reply = InferReply::Chat(ChatReply {
        text: "hi".into(),
        tool_calls: vec![],
        stop: StopReason::EndTurn,
        thought: None,
        usage,
        served: served(),
    });
    let script = Script {
        kind: RequestKind::Chat,
        steps: vec![
            ScriptStep::Emit(InferEvent::Routed(served())),
            ScriptStep::Emit(InferEvent::TextDelta("hi".into())),
            ScriptStep::Emit(InferEvent::Finished(reply.clone())),
        ],
    };
    let got = accounts(script)
        .infer(&need(), DataClass::Public, Tier::Fast, chat())
        .await;
    assert_eq!(got, Ok(reply));
}

#[tokio::test]
async fn a_refusal_is_an_error_the_app_shows() {
    let script = Script {
        kind: RequestKind::Chat,
        steps: vec![ScriptStep::Emit(InferEvent::Finished(InferReply::Refused(
            InferRefusal::NeedsGrant,
        )))],
    };
    let got = accounts(script)
        .infer(&need(), DataClass::Mail, Tier::Fast, chat())
        .await;
    assert_eq!(
        got,
        Err(ClientError::InferRefused(InferRefusal::NeedsGrant))
    );
}

#[tokio::test]
async fn a_session_streams_its_events_one_by_one() {
    let script = Script {
        kind: RequestKind::Chat,
        steps: vec![ScriptStep::Emit(InferEvent::TextDelta("a".into()))],
    };
    let mut session = accounts(script)
        .session(&need(), DataClass::Public, Tier::Fast)
        .await
        .expect("session");
    session
        .send(ClientFrame::Request(chat()))
        .await
        .expect("send");
    assert_eq!(session.next().await, Ok(InferEvent::TextDelta("a".into())));
}
