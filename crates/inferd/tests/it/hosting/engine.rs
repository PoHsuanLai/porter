//! A fake OpenAI-compatible engine on a Unix socket: the stand-in for vLLM and llama-server. It
//! answers `GET /health`, `POST /v1/chat/completions` (an SSE stream scripted per request) and
//! `POST /v1/embeddings` (a vector per input), and keeps every request body it was sent. One
//! connection serves one request, as `HttpClient` makes them.

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

/// What the engine says to one chat request.
#[derive(Debug, Clone)]
pub enum Chat {
    /// Text, one delta per piece.
    Say(Vec<&'static str>),
    /// One tool call with these arguments.
    Call {
        name: &'static str,
        arguments: String,
    },
    /// Reasoning only (`reasoning_content` deltas), then a stop: no text, no call.
    Think(Vec<&'static str>),
    /// Reasoning, then text.
    ThinkSay(Vec<&'static str>, Vec<&'static str>),
    /// An HTTP error with this status and no usable body.
    Fail(u16),
    /// Reads the request and never answers.
    Hang,
}

/// How the engine answers.
#[derive(Debug, Clone, Default)]
pub struct Script {
    /// Answers to chat requests, in order; the last one repeats.
    pub chat: Vec<Chat>,
    /// The width of the vectors it makes.
    pub dims: usize,
}

/// One request the engine received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// Header names in lower case, with their values.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Seen {
    /// The value of the header `name` (lower case), if the request had one.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(have, _)| have == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A running fake engine.
#[derive(Debug)]
pub struct FakeEngine {
    pub seen: Arc<Mutex<Vec<Seen>>>,
    task: JoinHandle<()>,
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeEngine {
    /// Listens on `socket`.
    pub fn start(socket: &Path, script: Script) -> Self {
        let listener = UnixListener::bind(socket).expect("bind the engine's socket");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new((VecDeque::from(script.chat), script.dims)));
        let log = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (state, log) = (Arc::clone(&state), Arc::clone(&log));
                tokio::spawn(async move { serve(stream, state, log).await });
            }
        });
        Self { seen, task }
    }

    /// The bodies of the requests to `path`, in order.
    pub fn bodies(&self, path: &str) -> Vec<Value> {
        self.seen
            .lock()
            .expect("lock")
            .iter()
            .filter(|seen| seen.path == path)
            .map(|seen| seen.body.clone())
            .collect()
    }
}

pub type State = Arc<Mutex<(VecDeque<Chat>, usize)>>;

async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Option<Seen> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&raw).into_owned();
        if let Some((head, _)) = text.split_once("\r\n\r\n") {
            let want = head
                .to_ascii_lowercase()
                .lines()
                .find_map(|line| line.strip_prefix("content-length:").map(str::to_owned))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            // The body is bytes: compare in bytes, not in the lossy text.
            let head_len = head.len() + 4;
            if raw.len() >= head_len + want {
                let mut first = head.lines().next()?.split(' ');
                let method = first.next()?.to_owned();
                let path = first.next()?.to_owned();
                let body =
                    serde_json::from_slice(&raw[head_len..head_len + want]).unwrap_or(Value::Null);
                let headers = head
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| {
                        (name.trim().to_ascii_lowercase(), value.trim().to_owned())
                    })
                    .collect();
                return Some(Seen {
                    method,
                    path,
                    headers,
                    body,
                });
            }
        }
    }
}

fn chunk(data: &str) -> Vec<u8> {
    format!("{:x}\r\n{data}\r\n", data.len()).into_bytes()
}

fn frame(delta: Value, finish: Value) -> String {
    let body = json!({"id": "c1", "object": "chat.completion.chunk", "model": "m",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    format!("data: {body}\n\n")
}

fn stream_for(answer: &Chat) -> Vec<Vec<u8>> {
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    let usage = "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n";
    let mut pieces = vec![head];
    match answer {
        Chat::Say(deltas) => {
            pieces.extend(
                deltas
                    .iter()
                    .map(|text| chunk(&frame(json!({"content": text}), Value::Null))),
            );
            pieces.push(chunk(&frame(json!({}), json!("stop"))));
        }
        Chat::Call { name, arguments } => {
            let delta = json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": name, "arguments": arguments}}]});
            pieces.push(chunk(&frame(delta, Value::Null)));
            pieces.push(chunk(&frame(json!({}), json!("tool_calls"))));
        }
        Chat::ThinkSay(thoughts, says) => {
            pieces.extend(
                thoughts
                    .iter()
                    .map(|text| chunk(&frame(json!({"reasoning_content": text}), Value::Null))),
            );
            pieces.extend(
                says.iter()
                    .map(|text| chunk(&frame(json!({"content": text}), Value::Null))),
            );
            pieces.push(chunk(&frame(json!({}), json!("stop"))));
        }
        Chat::Think(deltas) => {
            pieces.extend(
                deltas
                    .iter()
                    .map(|text| chunk(&frame(json!({"reasoning_content": text}), Value::Null))),
            );
            pieces.push(chunk(&frame(json!({}), json!("stop"))));
        }
        Chat::Fail(_) | Chat::Hang => {}
    }
    pieces.push(chunk(usage));
    pieces.push(b"0\r\n\r\n".to_vec());
    pieces
}

fn event(name: &str, data: &Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// What an Anthropic Messages server streams for `answer`: text deltas, or one tool_use block.
fn messages_stream(answer: &Chat) -> Vec<Vec<u8>> {
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    let mut events = vec![event(
        "message_start",
        &json!({"type": "message_start", "message": {"id": "msg_fake", "type": "message",
            "role": "assistant", "model": "m", "content": [],
            "usage": {"input_tokens": 5, "output_tokens": 1}}}),
    )];
    let stop = match answer {
        Chat::Call { name, arguments } => {
            events.push(event("content_block_start", &json!({"type": "content_block_start",
                "index": 0, "content_block": {"type": "tool_use", "id": "toolu_fake", "name": name, "input": {}}})));
            events.push(event(
                "content_block_delta",
                &json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": arguments}}),
            ));
            events.push(event(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": 0}),
            ));
            "tool_use"
        }
        Chat::Say(deltas) => {
            events.push(event(
                "content_block_start",
                &json!({"type": "content_block_start",
                "index": 0, "content_block": {"type": "text", "text": ""}}),
            ));
            for text in deltas {
                events.push(event(
                    "content_block_delta",
                    &json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": text}}),
                ));
            }
            events.push(event(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": 0}),
            ));
            "end_turn"
        }
        _ => "end_turn",
    };
    events.push(event(
        "message_delta",
        &json!({"type": "message_delta",
        "delta": {"stop_reason": stop, "stop_sequence": null}, "usage": {"output_tokens": 2}}),
    ));
    events.push(event("message_stop", &json!({"type": "message_stop"})));
    let mut pieces = vec![head];
    pieces.extend(events.iter().map(|e| chunk(e)));
    pieces.push(b"0\r\n\r\n".to_vec());
    pieces
}

/// The whole message a non-streaming Anthropic server answers.
fn messages_whole(answer: &Chat) -> Value {
    let content = match answer {
        Chat::Call { name, arguments } => {
            json!([{"type": "tool_use", "id": "toolu_fake", "name": name,
            "input": serde_json::from_str::<Value>(arguments).unwrap_or(Value::Null)}])
        }
        Chat::Say(deltas) => json!([{"type": "text", "text": deltas.concat()}]),
        _ => json!([]),
    };
    json!({"id": "msg_fake", "type": "message", "role": "assistant", "model": "m", "content": content,
        "stop_reason": if matches!(answer, Chat::Call { .. }) { "tool_use" } else { "end_turn" },
        "usage": {"input_tokens": 5, "output_tokens": 2}})
}

/// The whole completion a non-streaming chat-completions server answers.
fn completion_whole(answer: &Chat) -> Value {
    let message = match answer {
        Chat::Call { name, arguments } => json!({"role": "assistant", "content": null,
            "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": name, "arguments": arguments}}]}),
        Chat::Say(deltas) => json!({"role": "assistant", "content": deltas.concat()}),
        _ => json!({"role": "assistant", "content": ""}),
    };
    json!({"id": "c1", "object": "chat.completion", "model": "m",
        "choices": [{"index": 0, "message": message,
            "finish_reason": if matches!(answer, Chat::Call { .. }) { "tool_calls" } else { "stop" }}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2}})
}

fn json_response(status: &str, body: &Value) -> Vec<u8> {
    let body = body.to_string();
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// A vector for a text: its width, and numbers that depend on the text so two texts differ.
fn vector(text: &str, dims: usize) -> Vec<f64> {
    let sum: u64 = text.bytes().map(u64::from).sum();
    (0..dims)
        .map(|i| ((sum * (i as u64 + 1)) % 100) as f64 / 100.0)
        .collect()
}

pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    state: State,
    log: Arc<Mutex<Vec<Seen>>>,
) {
    let Some(seen) = read_request(&mut stream).await else {
        return;
    };
    log.lock().expect("lock").push(seen.clone());
    let pieces: Vec<Vec<u8>> = match (seen.method.as_str(), seen.path.as_str()) {
        ("GET", "/health") => vec![json_response("200 OK", &json!({"status": "ok"}))],
        ("POST", path) if path.ends_with("/chat/completions") || path.ends_with("/messages") => {
            let anthropic = path.ends_with("/messages");
            let streaming = seen.body["stream"] == json!(true);
            let answer = {
                let mut state = state.lock().expect("lock");
                match state.0.len() {
                    0 => Chat::Say(vec!["(unscripted)"]),
                    1 => state.0[0].clone(),
                    _ => state.0.pop_front().expect("one"),
                }
            };
            match answer {
                Chat::Hang => {
                    // Hold the connection until the client leaves.
                    let mut rest = [0_u8; 64];
                    while stream.read(&mut rest).await.is_ok_and(|n| n > 0) {}
                    return;
                }
                Chat::Fail(status) => vec![json_response(
                    &format!("{status} Error"),
                    &json!({"error": {"message": "scripted failure", "type": "server_error"}}),
                )],
                other if anthropic && streaming => messages_stream(&other),
                other if anthropic => vec![json_response("200 OK", &messages_whole(&other))],
                other if streaming => stream_for(&other),
                other => vec![json_response("200 OK", &completion_whole(&other))],
            }
        }
        ("POST", path) if path.ends_with("/embeddings") => {
            let dims = state.lock().expect("lock").1;
            let inputs: Vec<String> = seen.body["input"]
                .as_array()
                .map(|all| {
                    all.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .map(|(index, text)| json!({"index": index, "embedding": vector(text, dims)}))
                .collect();
            vec![json_response(
                "200 OK",
                &json!({"data": data, "usage": {"prompt_tokens": inputs.len()}}),
            )]
        }
        _ => vec![json_response(
            "404 Not Found",
            &json!({"error": {"message": "no such route"}}),
        )],
    };
    for piece in pieces {
        if stream.write_all(&piece).await.is_err() {
            return;
        }
        let _ = stream.flush().await;
    }
    let _ = stream.shutdown().await;
}
