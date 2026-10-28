//! Whole-body exchanges over scripted connections: reuse, the target and `Host` on the wire, the
//! reopen after a lapsed credential, a kept connection the server closed, and the response cap.

use crate::server::{After, Seen, serve};
use porter_http::http::{Method, Request, StatusCode};
use porter_http::{Http, HttpError, request};
use porter_http_relay::{Dial, RelayHttp, RelayLimits};
use porter_ids::WebUrl;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::io::DuplexStream;

const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
const LAPSED: &str = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";

/// A dial that opens in-memory connections, each served from a script (one answer per request).
struct Scripted {
    connections: Mutex<VecDeque<(Vec<String>, After)>>,
    counts: Counts,
}

/// What a test reads back: how many connections were dialled and what the peers saw.
#[derive(Debug, Clone, Default)]
struct Counts {
    dialled: Arc<AtomicUsize>,
    seen: Arc<Seen>,
}

impl Scripted {
    /// One connection per row: the answers its far end gives, in order, and what it does then.
    fn new(connections: &[(&[&str], After)]) -> (Self, Counts) {
        let counts = Counts::default();
        let rows = connections
            .iter()
            .map(|(answers, after)| (answers.iter().map(|a| (*a).to_owned()).collect(), *after))
            .collect();
        (
            Self {
                connections: Mutex::new(rows),
                counts: counts.clone(),
            },
            counts,
        )
    }
}

impl Dial for Scripted {
    type Stream = DuplexStream;

    async fn dial(&self) -> Result<DuplexStream, HttpError> {
        self.counts.dialled.fetch_add(1, Ordering::SeqCst);
        let (answers, after) = self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .ok_or(HttpError::Unreachable)?;
        let (end, peer) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve(peer, answers, after, Arc::clone(&self.counts.seen)));
        Ok(end)
    }
}

fn get(url: &str) -> Request<Vec<u8>> {
    request(Method::GET, &WebUrl::parse(url).expect("url"))
        .body(Vec::new())
        .expect("request")
}

#[tokio::test]
async fn a_kept_connection_serves_the_next_request_and_the_wire_has_a_target_and_a_host() {
    let (dial, counts) = Scripted::new(&[(&[OK, OK], After::Quiet)]);
    let http = RelayHttp::new(dial, RelayLimits::default());
    for _ in 0..2 {
        let answer = http
            .send(get("https://h.example.test:8443/dav/x?a=b"))
            .await
            .expect("answer");
        assert_eq!(answer.status(), StatusCode::OK);
    }
    assert_eq!(counts.dialled.load(Ordering::SeqCst), 1, "one connection");
    let heads = counts.seen.heads();
    assert_eq!(heads.len(), 2);
    assert!(
        heads[0].starts_with("GET /dav/x?a=b HTTP/1.1\r\n"),
        "{}",
        heads[0]
    );
    assert!(
        heads[0]
            .to_ascii_lowercase()
            .contains("host: h.example.test:8443\r\n"),
        "{}",
        heads[0]
    );
}

#[tokio::test]
async fn a_kept_connection_answered_401_is_dropped_and_the_request_goes_out_on_a_new_one() {
    let (dial, counts) = Scripted::new(&[
        (&[OK, LAPSED], After::Quiet),
        (&[OK, LAPSED], After::Quiet),
        (&[LAPSED], After::Quiet),
    ]);
    let http = RelayHttp::new(dial, RelayLimits::default());
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(
            http.send(get("https://h.example.test/x"))
                .await
                .expect("answer")
                .status(),
        );
    }
    // The second request finds its kept connection lapsed and succeeds on a new one; the third
    // finds the same and the new connection's 401 is the answer, not tried again.
    assert_eq!(
        seen,
        [StatusCode::OK, StatusCode::OK, StatusCode::UNAUTHORIZED]
    );
    assert_eq!(counts.dialled.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_kept_connection_the_server_closed_is_replaced_by_a_new_one() {
    let (dial, counts) = Scripted::new(&[(&[OK], After::Close), (&[OK], After::Quiet)]);
    let http = RelayHttp::new(dial, RelayLimits::default());
    for path in ["a", "b"] {
        let url = format!("https://h.example.test/{path}");
        assert_eq!(
            http.send(get(&url)).await.expect("answer").status(),
            StatusCode::OK,
            "{path}"
        );
    }
    assert_eq!(counts.dialled.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_response_over_the_cap_is_too_large() {
    let head = "HTTP/1.1 200 OK\r\nContent-Length: 2048\r\n\r\n";
    let answer = format!("{head}{}", "x".repeat(2048));
    let answers = [answer.as_str()];
    let (dial, _) = Scripted::new(&[(&answers, After::Quiet)]);
    let small = RelayHttp::new(
        dial,
        RelayLimits {
            max_body: 1024,
            ..RelayLimits::default()
        },
    );
    assert_eq!(
        small.send(get("https://h.example.test/")).await.err(),
        Some(HttpError::TooLarge)
    );
    let (dial, _) = Scripted::new(&[(&answers, After::Quiet)]);
    let roomy = RelayHttp::new(
        dial,
        RelayLimits {
            max_body: 2048,
            ..RelayLimits::default()
        },
    );
    let response = roomy
        .send(get("https://h.example.test/"))
        .await
        .expect("response");
    assert_eq!(response.body().len(), 2048);
}
