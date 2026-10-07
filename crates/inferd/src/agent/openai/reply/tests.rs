use super::*;
use porter_core::{AccountId, Locality, ModelId, Tokens};
use porter_infer::{JsonText, ServedBy, TokenUsage, ToolCallId, ToolName};

fn reply(
    text: &str,
    thought: Option<&str>,
    calls: &[(&str, &str, &str)],
    stop: StopReason,
) -> ChatReply {
    ChatReply {
        text: text.into(),
        tool_calls: calls
            .iter()
            .map(|(id, name, args)| ToolCallPart {
                id: ToolCallId((*id).into()),
                name: ToolName::parse(name).expect("name"),
                args: JsonText::parse(args).expect("json"),
            })
            .collect(),
        stop,
        thought: thought.map(str::to_owned),
        usage: TokenUsage {
            input: Tokens(12),
            output: Tokens(7),
            cached: Tokens(2),
        },
        served: ServedBy {
            account: AccountId::parse("local").expect("id"),
            model: ModelId::parse("m").expect("id"),
            locality: Locality::OnDevice,
        },
    }
}

#[test]
fn a_completion_has_null_content_beside_tool_calls_and_openai_usage() {
    let calls = [("c1", "run", r#"{"a":1}"#), ("c2", "run", "{}")];
    let body = completion_json(
        "gpt-x",
        "chatcmpl-1",
        5,
        &reply("", Some("hm"), &calls, StopReason::ToolUse),
    );
    let choice = &body["choices"][0];
    assert_eq!(choice["finish_reason"], "tool_calls");
    assert!(choice["message"]["content"].is_null());
    assert_eq!(choice["message"]["reasoning_content"], "hm");
    assert_eq!(choice["message"]["tool_calls"][1]["id"], "c2");
    assert_eq!(
        choice["message"]["tool_calls"][0]["function"]["arguments"],
        r#"{"a":1}"#
    );
    assert!(choice["message"]["tool_calls"][0].get("index").is_none());
    assert_eq!(body["usage"]["prompt_tokens"], 12);
    assert_eq!(body["usage"]["completion_tokens"], 7);
    assert_eq!(body["usage"]["total_tokens"], 19);
    assert_eq!(body["usage"]["prompt_tokens_details"]["cached_tokens"], 2);
}

#[test]
fn every_stop_reason_has_openais_word() {
    let cases = [
        (StopReason::EndTurn, "stop"),
        (StopReason::StopSequence, "stop"),
        (StopReason::ToolUse, "tool_calls"),
        (StopReason::MaxTokens, "length"),
        (StopReason::ContentFilter, "content_filter"),
    ];
    for (stop, word) in cases {
        assert_eq!(finish_word(stop), word);
    }
}

fn chunks(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter(|c| !c.is_empty() && *c != "data: [DONE]")
        .map(|c| serde_json::from_str(c.strip_prefix("data: ").expect("data")).expect("json"))
        .collect()
}

#[test]
fn a_stream_sends_role_deltas_indexed_tool_calls_a_finish_and_usage_then_done() {
    let calls = [("c1", "run", r#"{"a":1}"#), ("c2", "run", "{}")];
    let done = reply("hi", None, &calls, StopReason::ToolUse);
    let mut stream = Stream::new("gpt-x", "chatcmpl-1", 5, true);
    let mut out = stream.begin();
    out += &stream.thought("t");
    out += &stream.text("hi");
    out += &stream.tool(&done.tool_calls[0]);
    out += &stream.tool(&done.tool_calls[1]);
    out += &stream.finish(&done);
    assert!(out.ends_with("data: [DONE]\n\n"));
    let all = chunks(&out);
    assert_eq!(all[0]["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(all[1]["choices"][0]["delta"]["reasoning_content"], "t");
    assert_eq!(all[2]["choices"][0]["delta"]["content"], "hi");
    assert_eq!(all[3]["choices"][0]["delta"]["tool_calls"][0]["index"], 0);
    assert_eq!(all[4]["choices"][0]["delta"]["tool_calls"][0]["index"], 1);
    assert_eq!(all[4]["choices"][0]["delta"]["tool_calls"][0]["id"], "c2");
    assert_eq!(all[5]["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(all[6]["usage"]["completion_tokens"], 7);
    assert_eq!(all[6]["choices"], json!([]));
    let mut quiet = Stream::new("m", "id", 1, false);
    let out = quiet.finish(&done);
    assert!(!out.contains("usage"), "{out}");
}

#[test]
fn the_model_list_names_each_id() {
    let list = models_json(&["a".into(), "b".into()], 9);
    assert_eq!(list["object"], "list");
    assert_eq!(list["data"][1]["id"], "b");
    assert_eq!(list["data"][1]["object"], "model");
}
