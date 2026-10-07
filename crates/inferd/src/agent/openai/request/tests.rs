use super::*;
use porter_core::Tokens;
use porter_infer::Knob;
use serde_json::json;

fn read(body: &Value) -> Result<Parsed, Unmapped> {
    parse(body.to_string().as_bytes(), DataClass::Files)
}

#[test]
fn a_tool_conversation_maps_calls_and_joins_parallel_results() {
    let body = json!({
        "model": "gpt-x", "max_completion_tokens": 300, "stream": true,
        "stream_options": { "include_usage": true },
        "parallel_tool_calls": false, "tool_choice": "required",
        "tools": [{ "type": "function", "function": {
            "name": "run", "description": "runs", "parameters": { "type": "object" } } }],
        "messages": [
            { "role": "developer", "content": "be brief" },
            { "role": "user", "content": [{ "type": "text", "text": "do both" }] },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "c1", "type": "function", "function": { "name": "run", "arguments": "{\"a\":1}" } },
                { "id": "c2", "type": "function", "function": { "name": "run", "arguments": "" } },
            ]},
            { "role": "tool", "tool_call_id": "c1", "content": "one" },
            { "role": "tool", "tool_call_id": "c2", "content": "two" },
            { "role": "user", "content": "thanks" },
        ],
    });
    let parsed = read(&body).expect("parsed");
    assert!(parsed.stream && parsed.include_usage);
    let chat = parsed.chat;
    assert_eq!(chat.control.max_output, Knob::Set(Tokens(300)));
    assert_eq!(chat.control.tool_choice, ToolChoice::Required);
    assert_eq!(chat.control.tool_calls, ToolParallelism::One);
    let roles: Vec<Role> = chat.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            Role::System,
            Role::User,
            Role::Assistant,
            Role::User,
            Role::User
        ]
    );
    let MessagePart::ToolCall(second) = &chat.messages[2].parts[1] else {
        panic!("a call");
    };
    assert_eq!(
        second.args.as_str(),
        "{}",
        "empty arguments are an empty object"
    );
    let results: Vec<(&str, &str)> = chat.messages[3]
        .parts
        .iter()
        .map(|part| match part {
            MessagePart::ToolResult(r) => match r.parts.as_slice() {
                [MessagePart::Text(text)] => (r.id.0.as_str(), text.as_str()),
                other => panic!("one text, got {other:?}"),
            },
            other => panic!("a result, got {other:?}"),
        })
        .collect();
    assert_eq!(results, [("c1", "one"), ("c2", "two")]);
}

#[test]
fn a_data_url_image_is_read_and_a_web_url_is_refused() {
    let with = |url: &str| {
        json!({ "model": "m", "messages": [{ "role": "user", "content": [
            { "type": "image_url", "image_url": { "url": url } }] }] })
    };
    let chat = read(&with("data:image/png;base64,iVBORw0KGgo="))
        .expect("parsed")
        .chat;
    let MessagePart::Image(image) = &chat.messages[0].parts[0] else {
        panic!("an image");
    };
    assert_eq!(image.media_type, "image/png");
    assert!(read(&with("https://x/y.png")).is_err());
    assert!(read(&with("data:image/png,rawbytes")).is_err());
}

#[test]
fn reasoning_effort_response_format_and_stops_map_and_the_rest_is_refused_by_name() {
    let body = json!({
        "model": "m", "messages": [], "reasoning_effort": "high", "stop": "END",
        "response_format": { "type": "json_schema", "json_schema": { "name": "x", "schema": { "type": "object" } } },
        "temperature": 0.5, "store": true, "user": "u",
    });
    let chat = read(&body).expect("parsed").chat;
    assert_eq!(chat.control.reasoning, Reasoning::On(Effort::High));
    assert_eq!(chat.control.stop, ["END"]);
    assert_eq!(chat.shape, ReplyShape::Json(r#"{"type":"object"}"#.into()));
    let cases = [
        (
            json!({ "model": "m", "messages": [], "n": 2 }),
            "n over one",
        ),
        (
            json!({ "model": "m", "messages": [], "functions": [] }),
            "functions",
        ),
        (
            json!({ "model": "m", "messages": [], "response_format": { "type": "json_object" } }),
            "json_object",
        ),
        (
            json!({ "model": "m", "messages": [{ "role": "user", "content": [{ "type": "input_audio" }] }] }),
            "input_audio",
        ),
        (
            json!({ "model": "m", "messages": [{ "role": "function", "content": "x" }] }),
            "role",
        ),
        (json!({ "messages": [] }), "model"),
        (json!({ "model": "m" }), "messages"),
    ];
    for (body, word) in cases {
        let why = read(&body).expect_err(&body.to_string());
        assert!(why.0.contains(word), "{body} should name {word}: {}", why.0);
    }
}
