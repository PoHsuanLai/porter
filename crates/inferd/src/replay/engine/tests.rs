use super::*;
use crate::replay::cassette::{Cassette, TEST_HEADER};
use serde_json::json;
use tokio::net::UnixStream;

fn replayer() -> Replayer {
    let text = format!(
        "{TEST_HEADER}\n{}",
        r#"{"when":{},"reply":{"kind":"text","v":"pong"},"uses":"always"}"#
    );
    Replayer::new(Cassette::parse(&text).expect("cassette"))
}

fn request(method: &str, path: &str, body: Value) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        body,
    }
}

fn flat(pieces: Vec<Vec<u8>>) -> String {
    String::from_utf8_lossy(&pieces.concat()).into_owned()
}

#[test]
fn routes_answer_health_chat_and_nothing_else() {
    let played = replayer();
    assert!(
        flat(respond(&played, &request("GET", "/health", Value::Null))).starts_with("HTTP/1.1 200")
    );
    let chat = flat(respond(
        &played,
        &request(
            "POST",
            "/v1/chat/completions",
            json!({"stream": true, "messages": []}),
        ),
    ));
    assert!(chat.contains("pong"), "{chat}");
    assert!(
        flat(respond(
            &played,
            &request("POST", "/v1/embeddings", Value::Null)
        ))
        .starts_with("HTTP/1.1 404")
    );
}

#[test]
fn a_request_is_read_across_chunks() {
    let body = r#"{"a":1}"#;
    let raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(parsed(&raw.as_bytes()[..20]), None);
    assert_eq!(parsed(&raw.as_bytes()[..raw.len() - 1]), None);
    assert_eq!(
        parsed(raw.as_bytes()).map(|r| r.body),
        Some(json!({"a": 1}))
    );
}

#[tokio::test]
async fn the_server_answers_over_a_socket() {
    let (mut client, server) = UnixStream::pair().expect("pair");
    let played = Arc::new(replayer());
    let task = tokio::spawn(serve_one(server, played));
    client
        .write_all(b"GET /health HTTP/1.1\r\n\r\n")
        .await
        .expect("write");
    let mut out = String::new();
    client.read_to_string(&mut out).await.expect("read");
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    task.await.expect("served");
}
