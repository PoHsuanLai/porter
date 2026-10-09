//! An OpenAI Chat Completions reply from inferd's: a JSON completion, or the `data:` chunks of a
//! stream ending in `[DONE]`, and the model list.

use crate::agent::fail::{Failure, Shape};
use crate::agent::wire::{SseOut, counted};
use porter_core::Permille;
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

/// One option as OpenAI lists a token: the text, its log-probability and its bytes. A share of
/// nothing is OpenAI's own `-9999.0` ("not among the likely"), as JSON has no `-inf`.
fn token_json(option: &str, share: Permille) -> Value {
    let logprob = if share.0 == 0 {
        -9999.0
    } else {
        (f64::from(share.0) / 1000.0).ln()
    };
    json!({ "token": option, "logprob": logprob, "bytes": option.as_bytes() })
}

/// The `logprobs` of a `Choice` reply: the chosen option as the one token, with the declared
/// options that have a share, likeliest first, as its `top_logprobs` (at most `top` of them).
/// The numbers are the options' shares as natural logarithms (so rounded to thousandths and
/// renormalised over the declared options), not the engine's own token probabilities. `null`
/// when the engine gave no shares: the reply is the same without them.
fn logprobs_json(reply: &ChatReply, top: u32) -> Value {
    let Some(scores) = &reply.scores else {
        return Value::Null;
    };
    let mut ranked: Vec<_> = scores
        .as_slice()
        .iter()
        .filter(|one| one.share.0 > 0)
        .collect();
    // Stable, so options of equal share keep the order they were declared in.
    ranked.sort_by_key(|one| std::cmp::Reverse(one.share));
    let best = ranked
        .into_iter()
        .take(usize::try_from(top).unwrap_or(usize::MAX))
        .map(|one| token_json(&one.option, one.share))
        .collect::<Vec<_>>();
    let mut chosen = token_json(
        &reply.text,
        scores.share_of(&reply.text).unwrap_or(Permille(0)),
    );
    chosen["top_logprobs"] = Value::Array(best);
    json!({ "content": [chosen], "refusal": null })
}

/// The whole completion for a non-streaming request.
pub fn completion_json(model: &str, id: &str, created: i64, reply: &ChatReply) -> Value {
    completion_json_with(model, id, created, reply, None)
}

/// As [`completion_json`], and with `logprobs` on the choice when the request asked for them of
/// a `Choice` (`top_logprobs` is how many options to list beside the chosen one).
pub fn completion_json_with(
    model: &str,
    id: &str,
    created: i64,
    reply: &ChatReply,
    top_logprobs: Option<u32>,
) -> Value {
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
    let mut choice = json!({
        "index": 0,
        "message": message,
        "finish_reason": finish_word(reply.stop),
    });
    if let Some(top) = top_logprobs {
        choice["logprobs"] = logprobs_json(reply, top);
    }
    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [choice],
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
    /// How many options to list in the `logprobs` of a `Choice` reply; none writes no `logprobs`.
    top_logprobs: Option<u32>,
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
            top_logprobs: None,
        }
    }

    /// This stream with the `logprobs` of a `Choice` reply (one chunk before the last, because
    /// the checked reply arrives whole), listing `top` options beside the chosen one.
    pub fn with_logprobs(mut self, top: Option<u32>) -> Self {
        self.top_logprobs = top;
        self
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
        let mut out = String::new();
        if let Some(top) = self.top_logprobs {
            out.push_str(&self.chunk(
                json!([{
                    "index": 0,
                    "delta": {},
                    "logprobs": logprobs_json(reply, top),
                    "finish_reason": null,
                }]),
                None,
            ));
        }
        out.push_str(&self.chunk(
            json!([{ "index": 0, "delta": {}, "finish_reason": finish_word(reply.stop) }]),
            None,
        ));
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
