use super::*;
use porter_core::Tokens;
use porter_infer::{Effort, Knob};
use serde_json::json;

fn read(body: &Value) -> Result<Parsed, Unmapped> {
    parse(body.to_string().as_bytes(), DataClass::Files)
}

fn call(id: &str, name: &str, args: &str) -> MessagePart {
    MessagePart::ToolCall(ToolCallPart {
        id: ToolCallId(id.into()),
        name: ToolName::parse(name).expect("name"),
        args: JsonText::parse(args).expect("json"),
    })
}

#[test]
fn a_tool_conversation_maps_to_calls_and_results_in_order() {
    let body = json!({
        "model": "claude-sonnet-4",
        "max_tokens": 512,
        "system": [{ "type": "text", "text": "be brief", "cache_control": { "type": "ephemeral" } }],
        "tools": [{
            "name": "read_file",
            "description": "reads a file",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string" } } },
        }],
        "tool_choice": { "type": "tool", "name": "read_file", "disable_parallel_tool_use": true },
        "messages": [
            { "role": "user", "content": "open main.rs" },
            { "role": "assistant", "content": [
                { "type": "text", "text": "reading" },
                { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": { "path": "main.rs" } },
            ]},
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}" },
                { "type": "tool_result", "tool_use_id": "toolu_2", "is_error": true,
                  "content": [{ "type": "text", "text": "no such file" }] },
                { "type": "text", "text": "go on" },
            ]},
        ],
    });
    let parsed = read(&body).expect("parsed");
    assert_eq!(parsed.model, "claude-sonnet-4");
    assert!(!parsed.stream);
    let chat = parsed.chat;
    assert_eq!(chat.class, DataClass::Files);
    assert_eq!(chat.control.max_output, Knob::Set(Tokens(512)));
    assert_eq!(
        chat.control.tool_choice,
        ToolChoice::Named(ToolName::parse("read_file").expect("name"))
    );
    assert_eq!(chat.control.tool_calls, ToolParallelism::One);
    assert_eq!(chat.tools.len(), 1);
    assert_eq!(chat.tools[0].name.as_str(), "read_file");
    let roles: Vec<Role> = chat.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::System, Role::User, Role::Assistant, Role::User]
    );
    assert_eq!(
        chat.messages[0].parts,
        [MessagePart::Text("be brief".into())]
    );
    assert_eq!(
        chat.messages[2].parts,
        [
            MessagePart::Text("reading".into()),
            call("toolu_1", "read_file", r#"{"path":"main.rs"}"#),
        ]
    );
    assert_eq!(
        chat.messages[3].parts,
        [
            MessagePart::ToolResult(ToolResultPart {
                id: ToolCallId("toolu_1".into()),
                status: ToolStatus::Ok,
                parts: vec![MessagePart::Text("fn main() {}".into())],
            }),
            MessagePart::ToolResult(ToolResultPart {
                id: ToolCallId("toolu_2".into()),
                status: ToolStatus::Error,
                parts: vec![MessagePart::Text("no such file".into())],
            }),
            MessagePart::Text("go on".into()),
        ]
    );
}

#[test]
fn thinking_passes_with_its_signature_and_redacted_thinking_with_its_data() {
    let body = json!({
        "model": "m", "max_tokens": 10, "thinking": { "type": "enabled", "budget_tokens": 1500 },
        "messages": [
            { "role": "assistant", "content": [
                { "type": "thinking", "thinking": "hmm", "signature": "sig" },
                { "type": "thinking", "thinking": "unsigned" },
                { "type": "redacted_thinking", "data": "opaque" },
            ]},
        ],
    });
    let chat = read(&body).expect("parsed").chat;
    assert_eq!(chat.control.reasoning, Reasoning::On(Effort::Low));
    let seals: Vec<(String, ThoughtSeal)> = chat.messages[0]
        .parts
        .iter()
        .map(|part| match part {
            MessagePart::Thought(t) => (t.text.clone(), t.seal.clone()),
            other => panic!("a thought, got {other:?}"),
        })
        .collect();
    assert_eq!(
        seals,
        [
            (
                "hmm".into(),
                ThoughtSeal::Signed(SignatureText("sig".into()))
            ),
            ("unsigned".into(), ThoughtSeal::None),
            (
                String::new(),
                ThoughtSeal::Redacted(OpaqueText("opaque".into()))
            ),
        ]
    );
}

#[test]
fn a_base64_image_is_read_and_the_other_sources_are_refused() {
    let png = "iVBORw0KGgo=";
    let body = json!({ "model": "m", "max_tokens": 1, "messages": [{ "role": "user", "content": [
        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": png } },
    ]}]});
    let chat = read(&body).expect("parsed").chat;
    let MessagePart::Image(image) = &chat.messages[0].parts[0] else {
        panic!("an image");
    };
    assert_eq!(image.media_type, "image/png");
    assert!(matches!(&image.source, ImageSource::Inline(bytes) if bytes.0.len() == 8));
    let url = json!({ "model": "m", "max_tokens": 1, "messages": [{ "role": "user", "content": [
        { "type": "image", "source": { "type": "url", "url": "https://x/y.png" } },
    ]}]});
    assert!(read(&url).is_err());
}

#[test]
fn what_cannot_be_mapped_is_refused_by_name() {
    let msg = |content: Value| json!({ "model": "m", "max_tokens": 1, "messages": [{ "role": "user", "content": content }] });
    let cases: Vec<(Value, &str)> = vec![
        (
            msg(json!([{ "type": "document", "source": {} }])),
            "document",
        ),
        (
            msg(json!([{ "type": "web_search_tool_result" }])),
            "web_search_tool_result",
        ),
        (
            json!({ "model": "m", "max_tokens": 1, "messages": [], "tools": [{ "type": "bash_20250124", "name": "bash" }] }),
            "bash_20250124",
        ),
        (
            json!({ "model": "m", "max_tokens": 1, "messages": [], "mcp_servers": [{ "name": "x" }] }),
            "mcp_servers",
        ),
        (
            json!({ "model": "m", "max_tokens": 1, "messages": [{ "role": "tool", "content": "x" }] }),
            "role",
        ),
        (json!({ "max_tokens": 1, "messages": [] }), "model"),
        (json!({ "model": "m", "max_tokens": 1 }), "messages"),
        (
            json!({ "model": "m", "max_tokens": 1, "messages": [], "tools": [{ "name": "has space", "input_schema": {} }] }),
            "tool name",
        ),
    ];
    for (body, word) in cases {
        let why = read(&body).expect_err(&body.to_string());
        assert!(
            why.0.contains(word),
            "{} should name {word}: {}",
            body,
            why.0
        );
    }
    assert!(parse(b"not json", DataClass::Files).is_err());
}

#[test]
fn defaults_and_dropped_hints_change_nothing() {
    let body = json!({
        "model": "m", "max_tokens": 5, "stream": true, "metadata": { "user_id": "u" },
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi", "cache_control": {} }] }],
        "stop_sequences": ["END"], "temperature": 0.25, "top_k": 20,
    });
    let parsed = read(&body).expect("parsed");
    assert!(parsed.stream);
    let chat = parsed.chat;
    assert_eq!(chat.control.tool_choice, ToolChoice::Auto);
    assert_eq!(chat.control.tool_calls, ToolParallelism::Many);
    assert_eq!(chat.control.reasoning, Reasoning::EngineDefault);
    assert_eq!(chat.control.stop, ["END"]);
    let Knob::Set(sampling) = chat.control.sampling else {
        panic!("a sampling");
    };
    assert_eq!(sampling.temperature, porter_core::Permille(250));
    assert_eq!(sampling.top_k, Knob::Set(porter_core::Count(20)));
    assert_eq!(chat.messages.len(), 1);
}

#[test]
fn count_tokens_estimates_a_quarter_of_the_characters() {
    let body =
        json!({ "model": "m", "messages": [{ "role": "user", "content": "x".repeat(400) }] });
    let n = estimate_tokens(body.to_string().as_bytes()).expect("estimate");
    assert!((100..140).contains(&n), "{n}");
    assert_eq!(estimate_tokens(b"{"), None);
}
