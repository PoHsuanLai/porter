use super::*;
use porter_core::{AccountId, Locality, ModelId, Tokens};
use porter_infer::{JsonText, ServedBy, TokenUsage, ToolCallId, ToolName};

fn reply(
    text: &str,
    thought: Option<&str>,
    calls: &[(&str, &str, &str)],
    stop: StopReason,
) -> ChatReply {
    let made = ChatReply::new(
        text.into(),
        stop,
        TokenUsage {
            input: Tokens(12),
            output: Tokens(7),
            cached: Tokens(2),
        },
        ServedBy {
            account: AccountId::parse("local").expect("id"),
            model: ModelId::parse("m").expect("id"),
            locality: Locality::OnDevice,
        },
    )
    .with_tool_calls(
        calls
            .iter()
            .map(|(id, name, args)| ToolCallPart {
                id: ToolCallId((*id).into()),
                name: ToolName::parse(name).expect("name"),
                args: JsonText::parse(args).expect("json"),
            })
            .collect(),
    );
    match thought {
        Some(t) => made.with_thought(t.to_owned()),
        None => made,
    }
}

#[test]
fn a_message_carries_thinking_text_and_tool_use_in_order_with_anthropic_usage() {
    let calls = [("call_1", "read_file", r#"{"path":"a"}"#)];
    let message = message_json(
        "claude-x",
        "msg_1",
        &reply("answer", Some("hmm"), &calls, StopReason::ToolUse),
    );
    assert_eq!(message["type"], "message");
    assert_eq!(message["model"], "claude-x");
    assert_eq!(message["stop_reason"], "tool_use");
    let kinds: Vec<&str> = message["content"]
        .as_array()
        .expect("content")
        .iter()
        .map(|b| b["type"].as_str().expect("type"))
        .collect();
    assert_eq!(kinds, ["thinking", "text", "tool_use"]);
    assert_eq!(message["content"][2]["input"]["path"], "a");
    assert_eq!(message["content"][2]["id"], "call_1");
    assert_eq!(message["usage"]["input_tokens"], 10);
    assert_eq!(message["usage"]["output_tokens"], 7);
    assert_eq!(message["usage"]["cache_read_input_tokens"], 2);
}

#[test]
fn every_stop_reason_has_anthropics_word() {
    let cases = [
        (StopReason::EndTurn, "end_turn"),
        (StopReason::ToolUse, "tool_use"),
        (StopReason::MaxTokens, "max_tokens"),
        (StopReason::StopSequence, "stop_sequence"),
        (StopReason::ContentFilter, "refusal"),
    ];
    for (stop, word) in cases {
        assert_eq!(stop_word(stop), word);
    }
}

/// The events of a stream as (name, data) pairs.
fn events(text: &str) -> Vec<(String, Value)> {
    text.split("\n\n")
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| {
            let (name, data) = chunk.split_once('\n').expect("two lines");
            (
                name.strip_prefix("event: ").expect("event").to_owned(),
                serde_json::from_str(data.strip_prefix("data: ").expect("data")).expect("json"),
            )
        })
        .collect()
}

#[test]
fn a_stream_opens_and_closes_each_block_and_ends_with_usage() {
    let mut stream = Stream::new("claude-x", "msg_1");
    let calls = [("call_1", "t", r#"{"a":1}"#)];
    let done = reply("ab", Some("hm"), &calls, StopReason::ToolUse);
    let mut out = stream.begin();
    out += &stream.thought("h");
    out += &stream.thought("m");
    out += &stream.text("a");
    out += &stream.text("b");
    out += &stream.tool(&done.tool_calls[0]);
    out += &stream.finish(&done);
    let seen = events(&out);
    let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    let indexes: Vec<u64> = seen
        .iter()
        .filter(|(n, _)| n == "content_block_start")
        .map(|(_, d)| d["index"].as_u64().expect("index"))
        .collect();
    assert_eq!(indexes, [0, 1, 2]);
    assert_eq!(seen[1].1["content_block"]["type"], "thinking");
    assert_eq!(seen[2].1["delta"]["thinking"], "h");
    assert_eq!(seen[5].1["content_block"]["type"], "text");
    assert_eq!(seen[9].1["content_block"]["type"], "tool_use");
    assert_eq!(seen[9].1["content_block"]["id"], "call_1");
    assert_eq!(seen[10].1["delta"]["type"], "input_json_delta");
    assert_eq!(seen[10].1["delta"]["partial_json"], r#"{"a":1}"#);
    let last = &seen[12].1;
    assert_eq!(last["delta"]["stop_reason"], "tool_use");
    assert_eq!(last["usage"]["output_tokens"], 7);
}

#[test]
fn a_stream_failure_is_an_error_event_in_anthropics_shape() {
    let mut stream = Stream::new("m", "msg_1");
    let failure = Failure::new(crate::agent::fail::Cause::Upstream, "the model failed");
    let seen = events(&stream.error(&failure));
    assert_eq!(seen[0].0, "error");
    assert_eq!(seen[0].1["error"]["type"], "api_error");
}
