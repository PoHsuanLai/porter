//! An Anthropic Messages reply from inferd's: a JSON message, or the event stream an agent reads
//! (`message_start`, content blocks with their deltas, `message_delta`, `message_stop`).

use crate::agent::fail::{Failure, Shape};
use crate::agent::wire::{SseOut, args_value, counted};
use porter_infer::{ChatReply, StopReason, ToolCallPart};
use serde_json::{Value, json};

/// Anthropic's word for why a reply ended.
fn stop_word(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxTokens => "max_tokens",
        StopReason::StopSequence => "stop_sequence",
        StopReason::ContentFilter => "refusal",
        // a variant a newer porter adds: told as a refusal
        _ => "refusal",
    }
}

fn usage_json(reply: &ChatReply) -> Value {
    let (input, output, cached) = counted(reply.usage);
    json!({
        "input_tokens": input,
        "output_tokens": output,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": cached,
    })
}

/// The whole message for a non-streaming request. `model` is the id the agent named.
pub fn message_json(model: &str, id: &str, reply: &ChatReply) -> Value {
    let mut content = Vec::new();
    if let Some(thought) = reply.thought.as_deref().filter(|t| !t.is_empty()) {
        content.push(json!({ "type": "thinking", "thinking": thought, "signature": "" }));
    }
    if !reply.text.is_empty() {
        content.push(json!({ "type": "text", "text": reply.text }));
    }
    for call in &reply.tool_calls {
        content.push(json!({
            "type": "tool_use",
            "id": call.id.0,
            "name": call.name.as_str(),
            "input": args_value(call),
        }));
    }
    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_word(reply.stop),
        "stop_sequence": null,
        "usage": usage_json(reply),
    })
}

/// One server-sent event.
fn event(name: &str, data: &Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// The block a stream has open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Nothing,
    Thinking,
    Text,
}

/// The stream of one reply.
#[derive(Debug)]
pub struct Stream {
    model: String,
    id: String,
    open: Open,
    index: u32,
}

impl Stream {
    /// A stream for the reply `id` of the model id the agent named.
    pub fn new(model: &str, id: &str) -> Self {
        Self {
            model: model.to_owned(),
            id: id.to_owned(),
            open: Open::Nothing,
            index: 0,
        }
    }

    /// Closes the open block, if any.
    fn close(&mut self) -> String {
        if self.open == Open::Nothing {
            return String::new();
        }
        self.open = Open::Nothing;
        let out = event(
            "content_block_stop",
            &json!({ "type": "content_block_stop", "index": self.index }),
        );
        self.index += 1;
        out
    }

    fn open_as(&mut self, kind: Open, block: Value) -> String {
        if self.open == kind {
            return String::new();
        }
        let mut out = self.close();
        self.open = kind;
        out.push_str(&event(
            "content_block_start",
            &json!({ "type": "content_block_start", "index": self.index, "content_block": block }),
        ));
        out
    }

    fn delta(&self, delta: &Value) -> String {
        event(
            "content_block_delta",
            &json!({ "type": "content_block_delta", "index": self.index, "delta": delta }),
        )
    }
}

impl SseOut for Stream {
    fn begin(&mut self) -> String {
        event(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": { "input_tokens": 0, "output_tokens": 0 },
                },
            }),
        )
    }

    fn thought(&mut self, text: &str) -> String {
        let mut out = self.open_as(
            Open::Thinking,
            json!({ "type": "thinking", "thinking": "", "signature": "" }),
        );
        out.push_str(&self.delta(&json!({ "type": "thinking_delta", "thinking": text })));
        out
    }

    fn text(&mut self, text: &str) -> String {
        let mut out = self.open_as(Open::Text, json!({ "type": "text", "text": "" }));
        out.push_str(&self.delta(&json!({ "type": "text_delta", "text": text })));
        out
    }

    fn tool(&mut self, call: &ToolCallPart) -> String {
        let mut out = self.close();
        out.push_str(&event(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": self.index,
                "content_block": {
                    "type": "tool_use",
                    "id": call.id.0,
                    "name": call.name.as_str(),
                    "input": {},
                },
            }),
        ));
        out.push_str(
            &self.delta(&json!({ "type": "input_json_delta", "partial_json": call.args.as_str() })),
        );
        out.push_str(&event(
            "content_block_stop",
            &json!({ "type": "content_block_stop", "index": self.index }),
        ));
        self.index += 1;
        out
    }

    fn finish(&mut self, reply: &ChatReply) -> String {
        let mut out = self.close();
        out.push_str(&event(
            "message_delta",
            &json!({
                "type": "message_delta",
                "delta": { "stop_reason": stop_word(reply.stop), "stop_sequence": null },
                "usage": usage_json(reply),
            }),
        ));
        out.push_str(&event("message_stop", &json!({ "type": "message_stop" })));
        out
    }

    fn error(&mut self, failure: &Failure) -> String {
        event("error", &failure.body(Shape::Anthropic))
    }
}

#[cfg(test)]
mod tests;
