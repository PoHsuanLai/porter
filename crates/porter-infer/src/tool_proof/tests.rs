use super::*;
use crate::control::StopReason;
use crate::ids::ToolCallId;
use crate::reply::{ServedBy, TokenUsage};
use crate::request::ToolCallPart;
use porter_core::Locality;
use porter_core::{AccountId, ModelId};
use porter_ids::Tokens;

fn reply(calls: &[(&str, &str)]) -> ChatReply {
    let served = ServedBy {
        account: AccountId::parse("local").expect("account"),
        model: ModelId::parse("m").expect("model"),
        locality: Locality::OnDevice,
    };
    let usage = TokenUsage {
        input: Tokens(1),
        output: Tokens(1),
        cached: Tokens(0),
    };
    ChatReply::new("".to_owned(), StopReason::ToolUse, usage, served).with_tool_calls(
        calls
            .iter()
            .map(|(name, args)| ToolCallPart {
                id: ToolCallId("c".to_owned()),
                name: ToolName::parse(name).expect("name"),
                args: JsonText::parse(args).expect("json"),
            })
            .collect(),
    )
}

#[test]
fn the_canned_request_offers_the_one_tool_and_carries_no_person_data() {
    let request = probe_request().expect("parses");
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0].name.as_str(), PROBE_TOOL);
    assert_eq!(request.class, DataClass::Public);
}

#[test]
fn only_one_valid_call_to_the_tool_passes() {
    let cases = [
        (
            "one valid call",
            reply(&[(PROBE_TOOL, r#"{"word":"maple"}"#)]),
            ToolProof::Passed,
        ),
        ("no call", reply(&[]), ToolProof::Failed),
        (
            "another function",
            reply(&[("other", r#"{"word":"maple"}"#)]),
            ToolProof::Failed,
        ),
        (
            "a missing argument",
            reply(&[(PROBE_TOOL, "{}")]),
            ToolProof::Failed,
        ),
        (
            "a number for the string",
            reply(&[(PROBE_TOOL, r#"{"word":3}"#)]),
            ToolProof::Failed,
        ),
        (
            "an extra argument",
            reply(&[(PROBE_TOOL, r#"{"word":"a","x":1}"#)]),
            ToolProof::Failed,
        ),
        (
            "arguments that are not an object",
            reply(&[(PROBE_TOOL, "[]")]),
            ToolProof::Failed,
        ),
        (
            "two calls",
            reply(&[
                (PROBE_TOOL, r#"{"word":"a"}"#),
                (PROBE_TOOL, r#"{"word":"b"}"#),
            ]),
            ToolProof::Failed,
        ),
    ];
    for (name, got, expected) in cases {
        assert_eq!(judge(&got), expected, "{name}");
    }
}
