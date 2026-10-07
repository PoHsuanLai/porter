//! An OpenAI Chat Completions reply from inferd's: a JSON completion, or the `data:` chunks of a
//! stream ending in `[DONE]`, and the model list.

use crate::agent::fail::{Failure, Shape};
use crate::agent::wire::{SseOut, counted};
use porter_infer::{ChatReply, StopReason, ToolCallPart};
use serde_json::{Value, json};

/// OpenAI's word for why a reply ended.
fn finish_word(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn | StopReason::StopSequence => "stop",
        StopReason::ToolUse => "tool_calls",
        StopReason::MaxTokens => "length",
        StopReason::ContentFilter => "content_filter",
    }
}

fn usage_json(reply: &ChatReply) -> Value {
    let input = reply.usage.input.0;
    let output = reply.usage.output.0;
    let (_, _, cached) = counted(reply.usage);
    json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input.saturating_add(output),
        "prompt_tokens_details": { "cached_tokens": cached },
    })
}

fn call_json(index: usize, call: &ToolCallPart) -> Value {
    json!({
        "index": index,
        "id": call.id.0,
        "type": "function",
        "function": { "name": call.name.as_str(), "arguments": call.args.as_str() },
    })
}

/// The whole completion for a non-streaming request.
pub fn completion_json(model: &str, id: &str, created: i64, reply: &ChatReply) -> Value {
    let mut message = json!({
        "role": "assistant",
        "content": if reply.text.is_empty() && !reply.tool_calls.is_empty() {
            Value::Null
        } else {
            Value::String(reply.text.clone())
        },
    });
    if let Some(thought) = reply.thought.as_deref().filter(|t| !t.is_empty()) {
        message["reasoning_content"] = Value::String(thought.to_owned());
    }
    if !reply.tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(
            reply
                .tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| {
                    let mut one = call_json(index, call);
                    if let Some(map) = one.as_object_mut() {
                        map.remove("index");
                    }
                    one
                })
                .collect(),
        );
    }
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_word(reply.stop),
        }],
        "usage": usage_json(reply),
    })
}

/// `GET /v1/models`: the ids the route serves.
pub fn models_json(ids: &[String], created: i64) -> Value {
    json!({
        "object": "list",
        "data": ids
            .iter()
            .map(|id| json!({ "id": id, "object": "model", "created": created, "owned_by": "porter" }))
            .collect::<Vec<_>>(),
    })
}

/// The stream of one reply.
#[derive(Debug)]
pub struct Stream {
    model: String,
    id: String,
    created: i64,
    include_usage: bool,
    calls: usize,
}

impl Stream {
    /// A stream for the completion `id`. `include_usage` is the request's
    /// `stream_options.include_usage`.
    pub fn new(model: &str, id: &str, created: i64, include_usage: bool) -> Self {
        Self {
            model: model.to_owned(),
            id: id.to_owned(),
            created,
            include_usage,
            calls: 0,
        }
    }

    fn chunk(&self, choices: Value, usage: Option<Value>) -> String {
        let mut body = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": choices,
        });
        if let Some(usage) = usage {
            body["usage"] = usage;
        }
        format!("data: {body}\n\n")
    }

    fn delta(&self, delta: &Value) -> String {
        self.chunk(
            json!([{ "index": 0, "delta": delta, "finish_reason": null }]),
            None,
        )
    }
}

impl SseOut for Stream {
    fn begin(&mut self) -> String {
        self.delta(&json!({ "role": "assistant", "content": "" }))
    }

    fn thought(&mut self, text: &str) -> String {
        self.delta(&json!({ "reasoning_content": text }))
    }

    fn text(&mut self, text: &str) -> String {
        self.delta(&json!({ "content": text }))
    }

    fn tool(&mut self, call: &ToolCallPart) -> String {
        let out = self.delta(&json!({ "tool_calls": [call_json(self.calls, call)] }));
        self.calls += 1;
        out
    }

    fn finish(&mut self, reply: &ChatReply) -> String {
        let mut out = self.chunk(
            json!([{ "index": 0, "delta": {}, "finish_reason": finish_word(reply.stop) }]),
            None,
        );
        if self.include_usage {
            out.push_str(&self.chunk(json!([]), Some(usage_json(reply))));
        }
        out.push_str("data: [DONE]\n\n");
        out
    }

    fn error(&mut self, failure: &Failure) -> String {
        format!("data: {}\n\ndata: [DONE]\n\n", failure.body(Shape::OpenAi))
    }
}

#[cfg(test)]
mod tests;
