//! An Anthropic Messages request as inferd's chat request.

use crate::agent::wire::{Parsed, Unmapped, effort_for_budget, limit_of, sampling_of, text_of};
use porter_core::DataClass;
use porter_core::consent::Usage;
use porter_infer::{
    Base64Bytes, ChatControl, ChatMessage, ChatRequest, ImagePart, ImageSource, JsonSchemaText,
    JsonText, Knob, MessagePart, OpaqueText, Reasoning, ReplyShape, Role, SignatureText,
    ThoughtPart, ThoughtSeal, ToolCallId, ToolCallPart, ToolChoice, ToolDecl, ToolName,
    ToolParallelism, ToolResultPart, ToolStatus,
};
use serde_json::Value;

/// Anthropic's default temperature.
const DEFAULT_TEMPERATURE: f64 = 1.0;

fn image(block: &Value) -> Result<MessagePart, Unmapped> {
    let source = block.get("source").unwrap_or(&Value::Null);
    match text_of(source, "type") {
        Some("base64") => {
            let media = text_of(source, "media_type").unwrap_or_default().to_owned();
            let data = text_of(source, "data").unwrap_or_default().to_owned();
            let bytes = serde_json::from_value::<Base64Bytes>(Value::String(data))
                .map_err(|_| Unmapped::of("an image whose data is not base64"))?;
            Ok(MessagePart::Image(ImagePart {
                media_type: media,
                source: ImageSource::Inline(bytes),
            }))
        }
        other => Err(Unmapped::of(format!(
            "an image source of type {}: only base64 images are mapped",
            other.unwrap_or("none")
        ))),
    }
}

fn tool_result_parts(content: Option<&Value>) -> Result<Vec<MessagePart>, Unmapped> {
    match content {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(vec![MessagePart::Text(text.clone())]),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| match text_of(block, "type") {
                Some("text") => Ok(MessagePart::Text(
                    text_of(block, "text").unwrap_or_default().to_owned(),
                )),
                Some("image") => image(block),
                other => Err(Unmapped::of(format!(
                    "a tool result block of type {}",
                    other.unwrap_or("none")
                ))),
            })
            .collect(),
        Some(_) => Err(Unmapped::of(
            "a tool result whose content is not text or blocks",
        )),
    }
}

fn block(block: &Value) -> Result<MessagePart, Unmapped> {
    let kind = text_of(block, "type").unwrap_or_default();
    match kind {
        "text" => Ok(MessagePart::Text(
            text_of(block, "text").unwrap_or_default().to_owned(),
        )),
        "image" => image(block),
        "tool_use" => {
            let name = ToolName::parse(text_of(block, "name").unwrap_or_default())
                .map_err(|_| Unmapped::of("a tool_use whose name is not a tool name"))?;
            let input = block
                .get("input")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            Ok(MessagePart::ToolCall(ToolCallPart {
                id: ToolCallId(text_of(block, "id").unwrap_or_default().to_owned()),
                name,
                args: JsonText::parse(&input.to_string())
                    .map_err(|_| Unmapped::of("a tool_use whose input is not JSON"))?,
            }))
        }
        "tool_result" => Ok(MessagePart::ToolResult(ToolResultPart {
            id: ToolCallId(text_of(block, "tool_use_id").unwrap_or_default().to_owned()),
            status: if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            },
            parts: tool_result_parts(block.get("content"))?,
        })),
        "thinking" => Ok(MessagePart::Thought(ThoughtPart {
            text: text_of(block, "thinking").unwrap_or_default().to_owned(),
            seal: match text_of(block, "signature") {
                Some(signature) if !signature.is_empty() => {
                    ThoughtSeal::Signed(SignatureText(signature.to_owned()))
                }
                _ => ThoughtSeal::None,
            },
        })),
        "redacted_thinking" => Ok(MessagePart::Thought(ThoughtPart {
            text: String::new(),
            seal: ThoughtSeal::Redacted(OpaqueText(
                text_of(block, "data").unwrap_or_default().to_owned(),
            )),
        })),
        other => Err(Unmapped::of(format!("a content block of type {other}"))),
    }
}

fn parts(content: &Value) -> Result<Vec<MessagePart>, Unmapped> {
    match content {
        Value::String(text) => Ok(vec![MessagePart::Text(text.clone())]),
        Value::Array(blocks) => blocks.iter().map(block).collect(),
        _ => Err(Unmapped::of("message content that is not text or blocks")),
    }
}

fn system(value: &Value) -> Result<Option<ChatMessage>, Unmapped> {
    let texts = match value {
        Value::Null => return Ok(None),
        Value::String(text) => vec![MessagePart::Text(text.clone())],
        Value::Array(blocks) => blocks
            .iter()
            .map(|b| match text_of(b, "type") {
                Some("text") => Ok(MessagePart::Text(
                    text_of(b, "text").unwrap_or_default().to_owned(),
                )),
                other => Err(Unmapped::of(format!(
                    "a system block of type {}",
                    other.unwrap_or("none")
                ))),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(Unmapped::of("a system prompt that is not text or blocks")),
    };
    Ok(Some(ChatMessage {
        role: Role::System,
        parts: texts,
    }))
}

fn tools(value: Option<&Value>) -> Result<Vec<ToolDecl>, Unmapped> {
    let Some(Value::Array(all)) = value else {
        return Ok(Vec::new());
    };
    all.iter()
        .map(|tool| {
            if let Some(kind) = text_of(tool, "type").filter(|kind| *kind != "custom") {
                return Err(Unmapped::of(format!(
                    "a tool of type {kind}: server tools are not mapped"
                )));
            }
            let schema = tool
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
            Ok(ToolDecl {
                name: ToolName::parse(text_of(tool, "name").unwrap_or_default())
                    .map_err(|_| Unmapped::of("a tool whose name is not a tool name"))?,
                description: text_of(tool, "description").unwrap_or_default().to_owned(),
                params: JsonSchemaText(
                    JsonText::parse(&schema.to_string())
                        .map_err(|_| Unmapped::of("a tool whose schema is not JSON"))?,
                ),
            })
        })
        .collect()
}

fn choice(value: Option<&Value>) -> Result<(ToolChoice, ToolParallelism), Unmapped> {
    let Some(choice) = value else {
        return Ok((ToolChoice::Auto, ToolParallelism::Many));
    };
    let parallelism = if choice
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        == Some(true)
    {
        ToolParallelism::One
    } else {
        ToolParallelism::Many
    };
    let picked = match text_of(choice, "type") {
        Some("auto") | None => ToolChoice::Auto,
        Some("any") => ToolChoice::Required,
        Some("none") => ToolChoice::Never,
        Some("tool") => ToolChoice::Named(
            ToolName::parse(text_of(choice, "name").unwrap_or_default())
                .map_err(|_| Unmapped::of("a tool_choice whose name is not a tool name"))?,
        ),
        Some(other) => return Err(Unmapped::of(format!("a tool_choice of type {other}"))),
    };
    Ok((picked, parallelism))
}

fn reasoning(value: Option<&Value>) -> Reasoning {
    match value.and_then(|v| text_of(v, "type")) {
        Some("enabled") => value
            .and_then(|v| v.get("budget_tokens"))
            .and_then(Value::as_u64)
            .map_or(Reasoning::EngineDefault, effort_for_budget),
        Some("disabled") => Reasoning::Off,
        _ => Reasoning::EngineDefault,
    }
}

/// Reads the body of a `POST /v1/messages`.
pub fn parse(body: &[u8], class: DataClass) -> Result<Parsed, Unmapped> {
    let root: Value =
        serde_json::from_slice(body).map_err(|_| Unmapped::of("a body that is not JSON"))?;
    let model = text_of(&root, "model")
        .ok_or_else(|| Unmapped::of("a request with no model"))?
        .to_owned();
    for refused in ["mcp_servers", "output_format"] {
        let named = root.get(refused).is_some_and(|v| match v {
            Value::Null => false,
            Value::Array(all) => !all.is_empty(),
            _ => true,
        });
        if named {
            return Err(Unmapped::of(format!("{refused} is not mapped")));
        }
    }
    let mut messages: Vec<ChatMessage> = system(root.get("system").unwrap_or(&Value::Null))?
        .into_iter()
        .collect();
    let all = root
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| Unmapped::of("a request with no messages"))?;
    for message in all {
        let role = match text_of(message, "role") {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => {
                return Err(Unmapped::of(
                    "a message whose role is not user or assistant",
                ));
            }
        };
        messages.push(ChatMessage {
            role,
            parts: parts(message.get("content").unwrap_or(&Value::Null))?,
        });
    }
    let (tool_choice, tool_calls) = choice(root.get("tool_choice"))?;
    let stop = root
        .get("stop_sequences")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(Parsed {
        top_logprobs: None,
        model,
        stream: root.get("stream").and_then(Value::as_bool) == Some(true),
        include_usage: true,
        chat: ChatRequest {
            messages,
            shape: ReplyShape::Text,
            tier: porter_core::Tier::Balanced,
            class,
            usage: Usage::Interactive,
            tools: tools(root.get("tools"))?,
            control: ChatControl {
                tool_choice,
                tool_calls,
                max_output: limit_of(root.get("max_tokens").and_then(Value::as_u64)),
                reasoning: reasoning(root.get("thinking")),
                sampling: sampling_of(
                    root.get("temperature").and_then(Value::as_f64),
                    root.get("top_p").and_then(Value::as_f64),
                    root.get("top_k").and_then(Value::as_u64),
                    DEFAULT_TEMPERATURE,
                ),
                stop,
                scores: Knob::Off,
            },
        },
    })
}

/// What `POST /v1/messages/count_tokens` answers: an estimate, a quarter of the characters the
/// request carries in its messages, system prompt and tools (rounded up). Not the provider's
/// count, and the same for every route; a client that needs the exact figure has none to get here.
pub fn estimate_tokens(body: &[u8]) -> Option<u32> {
    let root: Value = serde_json::from_slice(body).ok()?;
    let chars: usize = ["system", "messages", "tools"]
        .iter()
        .filter_map(|field| root.get(*field))
        .map(|value| value.to_string().len())
        .sum();
    Some(u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests;
