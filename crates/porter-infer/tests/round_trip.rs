//! Every wire or stored type of porter-infer survives its serde form.

use porter_core::consent::Usage;
use porter_core::need::DimsNeed;
use porter_core::{
    AccountId, AppId, AppName, Bytes, Count, DataClass, Isolation, Locality, MicroUsd, ModelId,
    Permille, Tier, Tokens, UnixSeconds,
};
use porter_infer::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
    let json = serde_json::to_string(value).expect("serializes");
    assert_eq!(
        &serde_json::from_str::<T>(&json).expect("deserializes"),
        value,
        "{json}"
    );
}

fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("ollama").expect("id"),
        model: ModelId::parse("llama3.2").expect("id"),
        locality: Locality::OnDevice,
    }
}

fn app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Mail").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

#[test]
fn requests_round_trip() {
    let chat = ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            parts: vec![
                MessagePart::Text("hi".into()),
                MessagePart::Image(ImagePart {
                    media_type: "image/png".into(),
                    source: ImageSource::Inline(Base64Bytes(vec![1, 2])),
                }),
                MessagePart::Image(ImagePart {
                    media_type: "image/png".into(),
                    source: ImageSource::Attached(AttachIndex(0)),
                }),
                MessagePart::ToolCall(ToolCallPart {
                    id: ToolCallId("call-1".into()),
                    name: ToolName::parse("mail.thread.archive").expect("name"),
                    args: JsonText::parse("{\"thread\":\"t1\"}").expect("json"),
                }),
                MessagePart::ToolResult(ToolResultPart {
                    id: ToolCallId("call-1".into()),
                    status: ToolStatus::Ok,
                    parts: vec![MessagePart::Text("done".into())],
                }),
            ],
        }],
        shape: ReplyShape::Json("{}".into()),
        tier: Tier::Fast,
        class: DataClass::Mail,
        usage: Usage::Interactive,
        control: ChatControl {
            tool_choice: ToolChoice::Named(ToolName::parse("mail.thread.archive").expect("name")),
            tool_calls: ToolParallelism::One,
            max_output: Knob::Set(Tokens(1)),
            reasoning: Reasoning::On(Effort::Low),
            sampling: Knob::Set(Sampling {
                temperature: Permille(700),
                top_p: Knob::Set(Permille(950)),
                top_k: Knob::Set(Count(20)),
                min_p: Knob::Off,
                seed: Knob::Set(Seed(7)),
            }),
            stop: vec!["\n\n".into()],
        },
        tools: vec![ToolDecl {
            name: ToolName::parse("mail.thread.archive").expect("name"),
            description: "Archive a thread".into(),
            params: JsonSchemaText(JsonText::parse("{\"type\":\"object\"}").expect("json")),
        }],
    };
    round_trip(&InferRequest::Chat(chat));
    round_trip(&InferRequest::Embed(EmbedRequest {
        inputs: vec!["a".into()],
        role: EmbedRole::Document,
        dims: DimsNeed::Any,
        class: DataClass::Notes,
        usage: Usage::Background,
    }));
    round_trip(&InferRequest::Task(TaskRequest {
        task: Task::Summarise,
        input: "text".into(),
        class: DataClass::Mail,
        usage: Usage::Interactive,
    }));
}

#[test]
fn replies_and_records_round_trip() {
    let usage = TokenUsage {
        input: Tokens(10),
        output: Tokens(5),
        cached: Tokens(4),
    };
    round_trip(&InferReply::Chat(ChatReply {
        text: "ok".into(),
        tool_calls: vec![],
        stop: StopReason::MaxTokens,
        thought: Some("hm".into()),
        usage,
        served: served(),
    }));
    round_trip(&InferReply::Embed(EmbedReply {
        vectors: vec![EmbedVector(vec![0.5, -1.0])],
        usage,
        served: served(),
    }));
    round_trip(&InferReply::Refused(InferRefusal::RequiresCloud(
        DataClass::Photos,
    )));
    round_trip(&InferReply::Refused(InferRefusal::OverBudget));
    round_trip(&Policy::proposed());
    round_trip(&SpendCap {
        scope: SpendScope::App(app()),
        period: Period::Monthly,
        limit: MicroUsd(1_000_000),
        warn_at: Permille(800),
    });
    round_trip(&SpendVerdict::Warn);
    round_trip(&AuditEntry {
        at: UnixSeconds(1),
        app: app(),
        account: served().account,
        class: DataClass::Mail,
        model: served().model,
        locality: Locality::Cloud { region: None },
        usage,
        bytes_out: Bytes(512),
        images: Count(1),
        audio_ms: Count(1500),
        why: Some(porter_infer::Why::Warm),
        cost: Some(MicroUsd(42)),
    });
}
