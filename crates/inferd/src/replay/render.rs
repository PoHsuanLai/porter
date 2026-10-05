//! A reply as the bytes an OpenAI-compatible engine sends: a chunked server-sent-events stream
//! for a streaming request (what inferd always asks for), one JSON body for the other kind.

use super::cassette::{Call, Reply};
use super::replayer::ReplayError;
use model_replay::{WireBody, WireEnd, WireReply};
use serde_json::{Value, json};

/// How the client asked to be answered (`stream` in the request body).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Server-sent events.
    Stream,
    /// One JSON object.
    Whole,
}

impl Delivery {
    /// What a chat request body asks for.
    pub fn of(body: &Value) -> Self {
        match body.get("stream").and_then(Value::as_bool) {
            Some(true) => Delivery::Stream,
            _ => Delivery::Whole,
        }
    }
}

const SSE_HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";

/// A usage frame so the turn's token counts are not empty.
const USAGE: &str = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";

fn chunk(data: &str) -> Vec<u8> {
    format!("{:x}\r\n{data}\r\n", data.len()).into_bytes()
}

fn frame(delta: Value, finish: Value) -> String {
    let body = json!({"id": "replay", "object": "chat.completion.chunk", "model": "replay",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    format!("data: {body}\n\n")
}

fn json_response(status: u16, body: &Value) -> Vec<u8> {
    let body = body.to_string();
    format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        reason(status),
        body.len()
    )
    .into_bytes()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Replay",
    }
}

/// The bytes of an error body: the shape engines use, with the replay's slug as the `type`.
pub fn error(status: u16, slug: &str, message: &str) -> Vec<Vec<u8>> {
    let body = json!({"error": {"message": message, "type": slug, "code": status}});
    vec![json_response(status, &body)]
}

/// A request that no entry answers: a typed error, status 422.
pub fn miss(why: &ReplayError) -> Vec<Vec<u8>> {
    error(422, why.slug(), &why.to_string())
}

/// What `GET /health` answers.
pub fn health() -> Vec<Vec<u8>> {
    vec![json_response(200, &json!({"status": "ok"}))]
}

/// What any other route answers.
pub fn not_found() -> Vec<Vec<u8>> {
    error(404, "replay_no_route", "replay: no such route")
}

fn call_json(at: usize, call: &Call) -> Value {
    json!({"index": at, "id": format!("call_{at}"), "type": "function",
        "function": {"name": call.name, "arguments": call.arguments.to_string()}})
}

/// The pieces to write for `reply`, in order. A stream a wire reply says was cut ends without
/// its terminating chunk, so the client sees a broken connection.
pub fn reply(reply: &Reply, delivery: Delivery) -> Vec<Vec<u8>> {
    match (reply, delivery) {
        (Reply::Fail(status), _) => error(*status, "replay_scripted", "replay: scripted failure"),
        (Reply::Wire(wire), _) => wire_reply(wire),
        (Reply::Text(text), Delivery::Stream) => {
            stream(frame(json!({"content": text}), Value::Null), "stop")
        }
        (Reply::Calls(calls), Delivery::Stream) => {
            let deltas: Vec<Value> = calls
                .iter()
                .enumerate()
                .map(|(at, c)| call_json(at, c))
                .collect();
            stream(
                frame(json!({"tool_calls": deltas}), Value::Null),
                "tool_calls",
            )
        }
        (Reply::Text(text), Delivery::Whole) => {
            whole(json!({"role": "assistant", "content": text}), "stop")
        }
        (Reply::Calls(calls), Delivery::Whole) => {
            let made: Vec<Value> = calls
                .iter()
                .enumerate()
                .map(|(at, c)| call_json(at, c))
                .collect();
            whole(
                json!({"role": "assistant", "content": null, "tool_calls": made}),
                "tool_calls",
            )
        }
    }
}

fn stream(first: String, finish: &str) -> Vec<Vec<u8>> {
    vec![
        SSE_HEAD.as_bytes().to_vec(),
        chunk(&first),
        chunk(&frame(json!({}), json!(finish))),
        chunk(USAGE),
        b"0\r\n\r\n".to_vec(),
    ]
}

fn whole(message: Value, finish: &str) -> Vec<Vec<u8>> {
    let body = json!({"id": "replay", "object": "chat.completion", "model": "replay",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}});
    vec![json_response(200, &body)]
}

fn wire_reply(wire: &WireReply) -> Vec<Vec<u8>> {
    let status = wire.head.status.0;
    match &wire.body {
        WireBody::Whole(text) => {
            let body = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                reason(status),
                text.len()
            );
            vec![body.into_bytes()]
        }
        WireBody::Frames(frames) => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                reason(status)
            );
            let body = frames.iter().map(|one| {
                let event = one
                    .event
                    .as_ref()
                    .map(|e| format!("event: {}\n", e.0))
                    .unwrap_or_default();
                let data: String = one
                    .data
                    .split('\n')
                    .map(|line| format!("data: {line}\n"))
                    .collect();
                chunk(&format!("{event}{data}\n"))
            });
            let end = match wire.end {
                WireEnd::Complete => Some(b"0\r\n\r\n".to_vec()),
                WireEnd::Cut | WireEnd::Reset => None,
            };
            std::iter::once(head.into_bytes())
                .chain(body)
                .chain(end)
                .collect()
        }
    }
}

#[cfg(test)]
mod tests;
