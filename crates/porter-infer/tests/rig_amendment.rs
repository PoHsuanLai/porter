//! The wire types the rig amendment added: pinned JSON, round trips, and the redaction rules.

use porter_core::{Count, Permille, Tokens};
use porter_infer::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

fn pinned<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T, json: &str) {
    assert_eq!(serde_json::to_string(value).expect("serializes"), json);
    assert_eq!(
        &serde_json::from_str::<T>(json).expect("deserializes"),
        value
    );
}

#[test]
fn knobs_are_written_in_full() {
    pinned(&Knob::<Tokens>::Off, r#"{"kind":"off"}"#);
    pinned(&Knob::Set(Tokens(1)), r#"{"kind":"set","v":1}"#);
}

#[test]
fn chat_control_has_pinned_json() {
    let control = ChatControl {
        tool_choice: ToolChoice::Named(ToolName::parse("mail.search").expect("name")),
        tool_calls: ToolParallelism::One,
        max_output: Knob::Set(Tokens(1)),
        reasoning: Reasoning::On(Effort::Low),
        sampling: Knob::Set(Sampling {
            temperature: Permille(700),
            top_p: Knob::Off,
            top_k: Knob::Set(Count(20)),
            min_p: Knob::Off,
            seed: Knob::Set(Seed(7)),
        }),
        stop: vec!["END".into()],
    };
    pinned(
        &control,
        r#"{"tool_choice":{"kind":"named","v":"mail.search"},"tool_calls":"one","max_output":{"kind":"set","v":1},"reasoning":{"kind":"on","v":"low"},"sampling":{"kind":"set","v":{"temperature":700,"top_p":{"kind":"off"},"top_k":{"kind":"set","v":20},"min_p":{"kind":"off"},"seed":{"kind":"set","v":7}}},"stop":["END"]}"#,
    );
}

#[test]
fn the_closed_sets_have_stable_slugs() {
    pinned(&ToolChoice::Auto, r#"{"kind":"auto"}"#);
    pinned(&ToolChoice::Never, r#"{"kind":"never"}"#);
    pinned(&ToolChoice::Required, r#"{"kind":"required"}"#);
    pinned(&ToolParallelism::Many, r#""many""#);
    pinned(&Reasoning::EngineDefault, r#"{"kind":"engine_default"}"#);
    pinned(&Reasoning::Off, r#"{"kind":"off"}"#);
    for (stop, slug) in [
        (StopReason::EndTurn, "end_turn"),
        (StopReason::ToolUse, "tool_use"),
        (StopReason::MaxTokens, "max_tokens"),
        (StopReason::StopSequence, "stop_sequence"),
        (StopReason::ContentFilter, "content_filter"),
    ] {
        pinned(&stop, &format!("\"{slug}\""));
    }
    pinned(&EmbedRole::Query, r#""query""#);
    pinned(&EmbedRole::Document, r#""document""#);
}

#[test]
fn reply_shapes_include_choice() {
    pinned(&ReplyShape::Text, r#"{"kind":"text"}"#);
    pinned(
        &ReplyShape::Choice(vec!["allow".into(), "deny".into()]),
        r#"{"kind":"choice","v":["allow","deny"]}"#,
    );
}

#[test]
fn a_thought_part_carries_its_seal_and_redacts() {
    let part = MessagePart::Thought(ThoughtPart {
        text: "hm".into(),
        seal: ThoughtSeal::Signed(SignatureText("sig".into())),
    });
    pinned(
        &part,
        r#"{"kind":"thought","v":{"text":"hm","seal":{"kind":"signed","v":"sig"}}}"#,
    );
    pinned(&ThoughtSeal::None, r#"{"kind":"none"}"#);
    pinned(
        &ThoughtSeal::Redacted(OpaqueText("enc".into())),
        r#"{"kind":"redacted","v":"enc"}"#,
    );
    assert_eq!(
        format!("{:?}", SignatureText("secret".into())),
        "SignatureText(<6 bytes>)"
    );
}

#[test]
fn usage_and_reply_carry_the_new_facts() {
    let usage = TokenUsage {
        input: Tokens(10),
        output: Tokens(5),
        cached: Tokens(8),
    };
    pinned(&usage, r#"{"input":10,"output":5,"cached":8}"#);
}

#[test]
fn open_options_reserve_the_traceparent() {
    let parent =
        Traceparent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").expect("ok");
    pinned(
        &OpenOptions {
            traceparent: Some(parent),
            ..OpenOptions::default()
        },
        r#"{"traceparent":"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"}"#,
    );
    pinned(&OpenOptions::default(), r#"{"traceparent":null}"#);
    assert!(serde_json::from_str::<OpenOptions>(r#"{"traceparent":"nope"}"#).is_err());
}

#[test]
fn open_options_carry_the_usage_as_its_slug() {
    use porter_core::consent::Usage;
    pinned(
        &OpenOptions::default().with_usage(Usage::Background),
        r#"{"traceparent":null,"usage":"background"}"#,
    );
    pinned(
        &OpenOptions::default().with_usage(Usage::Interactive),
        r#"{"traceparent":null,"usage":"interactive"}"#,
    );
    // Absent is `None` and reads as Interactive; an older writer's JSON still reads.
    let old: OpenOptions = serde_json::from_str(r#"{"traceparent":null}"#).expect("old");
    assert_eq!(
        (old.usage, old.usage_or_default()),
        (None, Usage::Interactive)
    );
    assert!(serde_json::from_str::<OpenOptions>(r#"{"usage":"batch"}"#).is_err());
}
