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
    let chat = ChatRequest::new(
        vec![ChatMessage {
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
        Tier::Fast,
        DataClass::Mail,
        Usage::Interactive,
    )
    .with_shape(ReplyShape::Json("{}".into()))
    .with_control(
        ChatControl::new()
            .with_tool_choice(ToolChoice::Named(
                ToolName::parse("mail.thread.archive").expect("name"),
            ))
            .with_max_output(Knob::Set(Tokens(1)))
            .with_reasoning(Reasoning::On(Effort::Low))
            .with_sampling(Knob::Set(Sampling {
                temperature: Permille(700),
                top_p: Knob::Set(Permille(950)),
                top_k: Knob::Set(Count(20)),
                min_p: Knob::Off,
                seed: Knob::Set(Seed(7)),
            }))
            .with_stop(vec!["\n\n".into()])
            .with_scores(Knob::Set(ScoreOptions { top_k: Count(5) })),
    )
    .with_tools(vec![ToolDecl {
        name: ToolName::parse("mail.thread.archive").expect("name"),
        description: "Archive a thread".into(),
        params: JsonSchemaText(JsonText::parse("{\"type\":\"object\"}").expect("json")),
    }]);
    round_trip(&InferRequest::Chat(chat));
    round_trip(&InferRequest::Embed(EmbedRequest::new(
        vec!["a".into()],
        EmbedRole::Document,
        DimsNeed::Any,
        DataClass::Notes,
        Usage::Background,
    )));
    round_trip(&InferRequest::Task(TaskRequest::new(
        Task::Summarise,
        "text".into(),
        DataClass::Mail,
        Usage::Interactive,
    )));
}

#[test]
fn replies_and_records_round_trip() {
    let usage = TokenUsage {
        input: Tokens(10),
        output: Tokens(5),
        cached: Tokens(4),
    };
    round_trip(&InferReply::Chat(
        ChatReply::new("ok".into(), StopReason::MaxTokens, usage, served())
            .with_thought("hm".into())
            .with_scores(
                OptionScores::new(vec![
                    OptionScore {
                        option: "allow".into(),
                        share: Permille(750),
                    },
                    OptionScore {
                        option: "deny".into(),
                        share: Permille(250),
                    },
                ])
                .expect("a whole"),
            ),
    ));
    round_trip(&InferReply::Embed(EmbedReply::new(
        vec![EmbedVector(vec![0.5, -1.0])],
        usage,
        served(),
    )));
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

fn plain_chat_reply() -> ChatReply {
    ChatReply::new(
        "ok".into(),
        StopReason::EndTurn,
        TokenUsage {
            input: Tokens(1),
            output: Tokens(1),
            cached: Tokens(0),
        },
        served(),
    )
}

#[test]
fn a_reply_without_scores_writes_no_key_and_a_frame_from_before_them_reads() {
    let json = serde_json::to_value(plain_chat_reply()).expect("serializes");
    assert!(json.get("scores").is_none(), "{json}");
    let back: ChatReply = serde_json::from_value(json).expect("an older frame reads");
    assert_eq!(back.scores, None);
}

#[test]
fn the_scores_of_a_reply_are_a_list_of_options_with_their_shares() {
    let mut reply = plain_chat_reply();
    reply.scores = Some(
        OptionScores::new(vec![
            OptionScore {
                option: "allow".into(),
                share: Permille(750),
            },
            OptionScore {
                option: "deny".into(),
                share: Permille(250),
            },
        ])
        .expect("a whole"),
    );
    let json = serde_json::to_value(&reply).expect("serializes");
    assert_eq!(
        json["scores"],
        serde_json::json!([
            {"option": "allow", "share": 750},
            {"option": "deny", "share": 250},
        ])
    );
}

#[test]
fn the_score_knob_is_not_written_when_off_and_reads_when_absent() {
    let control = ChatControl::new();
    let json = serde_json::to_value(&control).expect("serializes");
    assert!(json.get("scores").is_none(), "{json}");
    let back: ChatControl = serde_json::from_value(json).expect("reads");
    assert_eq!(back, control);

    let asked = control.with_scores(Knob::Set(ScoreOptions::default()));
    let json = serde_json::to_value(&asked).expect("serializes");
    assert_eq!(
        json["scores"],
        serde_json::json!({"kind": "set", "v": {"top_k": 20}})
    );
    round_trip(&asked);
}
