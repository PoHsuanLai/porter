//! An OpenAI Chat Completions request as inferd's chat request.

use crate::agent::wire::{Parsed, Unmapped, limit_of, sampling_of, text_of};
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_infer::{
    Base64Bytes, ChatControl, ChatMessage, ChatRequest, Effort, ImagePart, ImageSource,
    JsonSchemaText, JsonText, MessagePart, Reasoning, ReplyShape, Role, ToolCallId, ToolCallPart,
    ToolChoice, ToolDecl, ToolName, ToolParallelism, ToolResultPart, ToolStatus,
};
use serde_json::Value;

/// OpenAI's default temperature.
const DEFAULT_TEMPERATURE: f64 = 1.0;

fn data_url(url: &str) -> Result<MessagePart, Unmapped> {
    let rest = url
        .strip_prefix("data:")
        .ok_or_else(|| Unmapped::of("an image URL that is not a data URL"))?;
    let (head, data) = rest
        .split_once(',')
        .ok_or_else(|| Unmapped::of("a malformed data URL"))?;
    let media = head
        .strip_suffix(";base64")
        .ok_or_else(|| Unmapped::of("a data URL that is not base64"))?;
    let bytes = serde_json::from_value::<Base64Bytes>(Value::String(data.to_owned()))
        .map_err(|_| Unmapped::of("an image whose data is not base64"))?;
    Ok(MessagePart::Image(ImagePart {
        media_type: media.to_owned(),
        source: ImageSource::Inline(bytes),
    }))
}

fn content(value: &Value) -> Result<Vec<MessagePart>, Unmapped> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![MessagePart::Text(text.clone())]),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match text_of(part, "type") {
                Some("text") => Ok(MessagePart::Text(
                    text_of(part, "text").unwrap_or_default().to_owned(),
                )),
                Some("image_url") => data_url(
                    part.get("image_url")
                        .and_then(|image| text_of(image, "url"))
                        .unwrap_or_default(),
                ),
                other => Err(Unmapped::of(format!(
                    "a content part of type {}",
                    other.unwrap_or("none")
                ))),
            })
            .collect(),
        _ => Err(Unmapped::of("message content that is not text or parts")),
    }
}

fn text_only(parts: Vec<MessagePart>) -> Vec<MessagePart> {
    parts
        .into_iter()
        .filter(|part| matches!(part, MessagePart::Text(_)))
        .collect()
}

fn call(call: &Value) -> Result<MessagePart, Unmapped> {
    let function = call.get("function").unwrap_or(&Value::Null);
    let arguments = match text_of(function, "arguments") {
        Some("") | None => "{}",
        Some(text) => text,
    };
    Ok(MessagePart::ToolCall(ToolCallPart {
        id: ToolCallId(text_of(call, "id").unwrap_or_default().to_owned()),
        name: ToolName::parse(text_of(function, "name").unwrap_or_default())
            .map_err(|_| Unmapped::of("a tool call whose name is not a tool name"))?,
        args: JsonText::parse(arguments)
            .map_err(|_| Unmapped::of("a tool call whose arguments are not JSON"))?,
    }))
}

/// The messages of a request. Tool messages that follow one another become one user message of
/// tool results, so a reply to several parallel calls stays together.
fn messages(all: &[Value]) -> Result<Vec<ChatMessage>, Unmapped> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for message in all {
        let body = message.get("content").unwrap_or(&Value::Null);
        match text_of(message, "role") {
            Some("system" | "developer") => out.push(ChatMessage {
                role: Role::System,
                parts: text_only(content(body)?),
            }),
            Some("user") => out.push(ChatMessage {
                role: Role::User,
                parts: content(body)?,
            }),
            Some("assistant") => {
                let mut parts = text_only(content(body)?);
                if let Some(Value::Array(calls)) = message.get("tool_calls") {
                    for one in calls {
                        parts.push(call(one)?);
                    }
                }
                out.push(ChatMessage {
                    role: Role::Assistant,
                    parts,
                });
            }
            Some("tool") => {
                let result = MessagePart::ToolResult(ToolResultPart {
                    id: ToolCallId(
                        text_of(message, "tool_call_id")
                            .unwrap_or_default()
                            .to_owned(),
                    ),
                    status: ToolStatus::Ok,
                    parts: content(body)?,
                });
                let joins = out.last().is_some_and(|last| {
                    last.role == Role::User
                        && last
                            .parts
                            .iter()
                            .all(|part| matches!(part, MessagePart::ToolResult(_)))
                });
                match out.last_mut() {
                    Some(last) if joins => last.parts.push(result),
                    _ => out.push(ChatMessage {
                        role: Role::User,
                        parts: vec![result],
                    }),
                }
            }
            other => {
                return Err(Unmapped::of(format!(
                    "a message with role {}",
                    other.unwrap_or("none")
                )));
            }
        }
    }
    Ok(out)
}

fn tools(value: Option<&Value>) -> Result<Vec<ToolDecl>, Unmapped> {
    let Some(Value::Array(all)) = value else {
        return Ok(Vec::new());
    };
    all.iter()
        .map(|tool| {
            if text_of(tool, "type") != Some("function") {
                return Err(Unmapped::of("a tool that is not a function"));
            }
            let function = tool.get("function").unwrap_or(&Value::Null);
            let schema = function
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
            Ok(ToolDecl {
                name: ToolName::parse(text_of(function, "name").unwrap_or_default())
                    .map_err(|_| Unmapped::of("a tool whose name is not a tool name"))?,
                description: text_of(function, "description")
                    .unwrap_or_default()
                    .to_owned(),
                params: JsonSchemaText(
                    JsonText::parse(&schema.to_string())
                        .map_err(|_| Unmapped::of("a tool whose schema is not JSON"))?,
                ),
            })
        })
        .collect()
}

fn choice(value: Option<&Value>) -> Result<ToolChoice, Unmapped> {
    match value {
        None | Some(Value::Null) => Ok(ToolChoice::Auto),
        Some(Value::String(word)) => match word.as_str() {
            "auto" => Ok(ToolChoice::Auto),
            "none" => Ok(ToolChoice::Never),
            "required" => Ok(ToolChoice::Required),
            other => Err(Unmapped::of(format!("a tool_choice of {other}"))),
        },
        Some(object) => Ok(ToolChoice::Named(
            ToolName::parse(
                object
                    .get("function")
                    .and_then(|f| text_of(f, "name"))
                    .unwrap_or_default(),
            )
            .map_err(|_| Unmapped::of("a tool_choice whose name is not a tool name"))?,
        )),
    }
}

fn reasoning(value: Option<&Value>) -> Reasoning {
    match value.and_then(Value::as_str) {
        Some("low" | "minimal") => Reasoning::On(Effort::Low),
        Some("medium") => Reasoning::On(Effort::Medium),
        Some("high") => Reasoning::On(Effort::High),
        Some("none") => Reasoning::Off,
        _ => Reasoning::EngineDefault,
    }
}

fn shape(value: Option<&Value>) -> Result<ReplyShape, Unmapped> {
    let Some(format) = value else {
        return Ok(ReplyShape::Text);
    };
    match text_of(format, "type") {
        Some("text") | None => Ok(ReplyShape::Text),
        Some("json_schema") => {
            let schema = format
                .get("json_schema")
                .and_then(|s| s.get("schema"))
                .ok_or_else(|| Unmapped::of("a json_schema response_format with no schema"))?;
            Ok(ReplyShape::Json(schema.to_string()))
        }
        Some(other) => Err(Unmapped::of(format!("a response_format of {other}"))),
    }
}

/// Reads the body of a `POST /v1/chat/completions`.
pub fn parse(body: &[u8], class: DataClass) -> Result<Parsed, Unmapped> {
    let root: Value =
        serde_json::from_slice(body).map_err(|_| Unmapped::of("a body that is not JSON"))?;
    let model = text_of(&root, "model")
        .ok_or_else(|| Unmapped::of("a request with no model"))?
        .to_owned();
    if root.get("n").and_then(Value::as_u64).is_some_and(|n| n > 1) {
        return Err(Unmapped::of("n over one"));
    }
    if root.get("functions").is_some() {
        return Err(Unmapped::of("the legacy functions field"));
    }
    let stop = match root.get("stop") {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(all)) => all
            .iter()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    let parallel = root.get("parallel_tool_calls").and_then(Value::as_bool);
    Ok(Parsed {
        model,
        stream: root.get("stream").and_then(Value::as_bool) == Some(true),
        include_usage: root
            .get("stream_options")
            .and_then(|o| o.get("include_usage"))
            .and_then(Value::as_bool)
            == Some(true),
        chat: ChatRequest {
            messages: messages(
                root.get("messages")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Unmapped::of("a request with no messages"))?,
            )?,
            shape: shape(root.get("response_format"))?,
            tier: porter_core::Tier::Balanced,
            class,
            usage: Usage::Interactive,
            tools: tools(root.get("tools"))?,
            control: ChatControl {
                tool_choice: choice(root.get("tool_choice"))?,
                tool_calls: if parallel == Some(false) {
                    ToolParallelism::One
                } else {
                    ToolParallelism::Many
                },
                max_output: limit_of(
                    root.get("max_completion_tokens")
                        .or_else(|| root.get("max_tokens"))
                        .and_then(Value::as_u64),
                ),
                reasoning: reasoning(root.get("reasoning_effort")),
                sampling: sampling_of(
                    root.get("temperature").and_then(Value::as_f64),
                    root.get("top_p").and_then(Value::as_f64),
                    None,
                    DEFAULT_TEMPERATURE,
                ),
                stop,
            },
        },
    })
}

#[cfg(test)]
mod tests;
