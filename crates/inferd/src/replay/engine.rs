//! The replay engine's server: an OpenAI-compatible HTTP server on a Unix socket, in this
//! process, that answers `GET /health` and `POST /v1/chat/completions` from a [`Replayer`]. It is
//! what stands where llama-server or vLLM would, behind the same socket path, so the real
//! router, supervisor driver, runner and codec run unchanged. One connection serves one request,
//! as `HttpClient` makes them. It opens no network socket and no file but the cassette's.

use super::render::{self, Delivery};
use super::replayer::Replayer;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// One request as far as the engine reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The path.
    pub path: String,
    /// The body as JSON; `Null` when there is none or it is not JSON.
    pub body: Value,
}

/// Reads one request: the head, then as many body bytes as `Content-Length` says.
pub async fn read_request(stream: &mut UnixStream) -> Option<Request> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(request) = parsed(&raw) {
            return Some(request);
        }
    }
}

/// The request in `raw`, once all of it has arrived.
fn parsed(raw: &[u8]) -> Option<Request> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let want = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:").map(str::to_owned))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let body = raw.get(split + 4..split + 4 + want)?;
    let mut first = head.lines().next()?.split(' ');
    Some(Request {
        method: first.next()?.to_owned(),
        path: first.next()?.to_owned(),
        body: serde_json::from_slice(body).unwrap_or(Value::Null),
    })
}

/// The pieces to write back for one request.
pub fn respond(replayer: &Replayer, request: &Request) -> Vec<Vec<u8>> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => render::health(),
        ("POST", "/v1/chat/completions") => match replayer.answer(&request.body) {
            Ok(reply) => render::reply(&reply, Delivery::of(&request.body)),
            Err(why) => render::miss(&why),
        },
        _ => render::not_found(),
    }
}

async fn serve_one(mut stream: UnixStream, replayer: Arc<Replayer>) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    for piece in respond(&replayer, &request) {
        if stream.write_all(&piece).await.is_err() {
            return;
        }
    }
    let _ = stream.shutdown().await;
}

/// Accepts connections on `listener` until the task is dropped or aborted.
pub async fn serve(listener: UnixListener, replayer: Arc<Replayer>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(serve_one(stream, Arc::clone(&replayer)));
    }
}

#[cfg(test)]
mod tests;
