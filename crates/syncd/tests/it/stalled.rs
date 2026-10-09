//! A server that stops answering ends the exchange (reliability finding rel-3): the relay stream
//! gives up after its idle time and the HTTP client reports a timeout, instead of the dataset
//! waiting on a half-open connection forever. No bus: a socket pair stands for the relay. The
//! tests that count time run on the paused clock over an in-memory stream (the idle time and
//! the pieces' spacing are virtual; with no kernel I/O the clock can only advance when every
//! task waits on a timer), so a loaded machine cannot make a gap longer than the idle time.

use porter_core::WebUrl;
use porter_core::stream::{ByteStream, duplex};
use porter_http::{Http, HttpError, HttpRequest, Method};
use std::sync::Mutex;
use std::time::Duration;
use storage_webdav::{Dial, StreamHttp, StreamLimits};
use syncd::webdav::RelayStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::Instant;

/// Hands out the one stream it was given; a second dial finds none and is unreachable.
struct Once {
    stream: Mutex<Option<RelayStream>>,
}

impl Once {
    fn new(stream: RelayStream) -> Self {
        Self {
            stream: Mutex::new(Some(stream)),
        }
    }
}

impl Dial for Once {
    type Stream = RelayStream;

    async fn dial(&self) -> Result<RelayStream, HttpError> {
        self.stream
            .lock()
            .expect("lock")
            .take()
            .ok_or(HttpError::Unreachable)
    }
}

fn get() -> HttpRequest {
    HttpRequest::new(
        Method::Get,
        WebUrl::parse("http://127.0.0.1:9/dav/").expect("url"),
    )
}

const IDLE: Duration = Duration::from_millis(200);

#[tokio::test(start_paused = true)]
async fn a_server_that_never_answers_ends_the_request_as_timed_out() {
    let (ours, mut theirs) = duplex(64 * 1024);
    let http = StreamHttp::new(
        Once::new(RelayStream::from_memory(ours).with_idle(IDLE)),
        StreamLimits::default(),
    );
    // The server reads the request and then says nothing, holding the connection open.
    let silent = tokio::spawn(async move {
        let mut request = [0u8; 1024];
        let _ = theirs.read(&mut request).await;
        std::future::pending::<()>().await;
    });
    let started = Instant::now();
    let got = http.send(get()).await;
    assert_eq!(got.expect_err("no answer"), HttpError::TimedOut);
    assert!(started.elapsed() >= IDLE, "it waited the idle time");
    assert!(started.elapsed() < Duration::from_secs(10));
    silent.abort();
}

#[tokio::test]
async fn a_body_that_stalls_part_way_ends_the_request_as_timed_out() {
    let (ours, mut theirs) = UnixStream::pair().expect("pair");
    let http = StreamHttp::new(
        Once::new(RelayStream::from_unix(ours).with_idle(IDLE)),
        StreamLimits::default(),
    );
    let stalls = tokio::spawn(async move {
        let mut request = [0u8; 1024];
        let _ = theirs.read(&mut request).await;
        theirs
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")
            .await
            .expect("write");
        std::future::pending::<()>().await;
    });
    let got = http.send(get()).await;
    assert_eq!(got.expect_err("stalled"), HttpError::TimedOut);
    stalls.abort();
}

#[tokio::test(start_paused = true)]
async fn a_slow_server_that_keeps_sending_is_not_cut_off_by_the_idle_time() {
    let (ours, mut theirs) = duplex(64 * 1024);
    let http = StreamHttp::new(
        Once::new(RelayStream::from_memory(ours).with_idle(IDLE)),
        StreamLimits::default(),
    );
    // Four pieces, 120 ms apart: 480 ms in all, more than the idle time, but never silent for it.
    let slow = tokio::spawn(async move {
        let mut request = [0u8; 1024];
        let _ = theirs.read(&mut request).await;
        for piece in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\n"[..],
            b"ab",
            b"cd",
            b"ef",
        ] {
            tokio::time::sleep(Duration::from_millis(120)).await;
            theirs.write_all(piece).await.expect("write");
        }
        std::future::pending::<()>().await;
    });
    let got = http.send(get()).await.expect("answered");
    assert_eq!(got.body, b"abcdef");
    slow.abort();
}

#[tokio::test]
async fn a_timed_out_connection_is_not_sent_the_request_again_on_a_new_one() {
    let (ours, mut theirs) = UnixStream::pair().expect("pair");
    let dial = Once::new(RelayStream::from_unix(ours).with_idle(IDLE));
    let http = StreamHttp::new(dial, StreamLimits::default());
    let answers_once = tokio::spawn(async move {
        let mut request = [0u8; 1024];
        let _ = theirs.read(&mut request).await;
        theirs
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("write");
        // Then it goes quiet with the connection kept.
        let _ = theirs.read(&mut request).await;
        std::future::pending::<()>().await;
    });
    http.send(get()).await.expect("first answer");
    // The kept connection now hangs: the request fails as timed out, and is not retried (a retry
    // would dial again, find no stream and fail as unreachable instead).
    let got = http.send(get()).await;
    assert_eq!(got.expect_err("stalled"), HttpError::TimedOut);
    answers_once.abort();
}
