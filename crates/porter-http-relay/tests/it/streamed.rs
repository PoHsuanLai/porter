//! Streamed bodies over a relay: a request body larger than the whole-body cap goes out as it is
//! produced and is never held, and a response is read piece by piece.

use crate::server::{Seen, read_request};
use hyper::body::{Body, Bytes, Frame, SizeHint};
use porter_core::WebUrl;
use porter_http::http::{Method, StatusCode};
use porter_http::{Http, HttpError, request};
use porter_http_relay::{Dial, RelayHttp, RelayLimits, RelayStream};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Hands out the one stream it was given.
struct Once(Mutex<Option<RelayStream>>);

impl Once {
    fn new(stream: RelayStream) -> Self {
        Self(Mutex::new(Some(stream)))
    }
}

impl Dial for Once {
    type Stream = RelayStream;

    async fn dial(&self) -> Result<RelayStream, HttpError> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(HttpError::Unreachable)
    }
}

const PIECE: usize = 64 * 1024;
static ZEROES: [u8; PIECE] = [0; PIECE];

/// A body of `total` zero bytes made piece by piece, which counts what it has made and notes the
/// most it has made beyond what the server has read: the bytes in flight, the only ones a
/// streaming client can be holding.
struct Counting {
    total: u64,
    made: u64,
    produced: Arc<AtomicU64>,
    consumed: Arc<AtomicU64>,
    peak: Arc<AtomicU64>,
}

impl Body for Counting {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if self.made >= self.total {
            return Poll::Ready(None);
        }
        let n = usize::try_from(self.total - self.made)
            .unwrap_or(PIECE)
            .min(PIECE);
        self.made += n as u64;
        let produced = self.produced.fetch_add(n as u64, Ordering::SeqCst) + n as u64;
        let in_flight = produced.saturating_sub(self.consumed.load(Ordering::SeqCst));
        self.peak.fetch_max(in_flight, Ordering::SeqCst);
        Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(&ZEROES[..n])))))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.total - self.made)
    }
}

#[tokio::test]
async fn an_upload_larger_than_the_cap_is_streamed_and_never_held() {
    // More than the 256 MiB a response may hold whole: a client that buffered it would hold it all.
    const TOTAL: u64 = 300 * 1024 * 1024;
    let (ours, mut theirs) = UnixStream::pair().expect("pair");
    let produced = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU64::new(0));
    let seen = Arc::new(Seen::default());
    let consumed = Arc::clone(&seen.body_bytes);
    let server = {
        let seen = Arc::clone(&seen);
        tokio::spawn(async move {
            // The server reads the head and then the body piece by piece, noting each piece.
            let mut buf = Vec::new();
            let head = read_request(&mut theirs, &mut buf, &seen)
                .await
                .expect("a request");
            theirs
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok")
                .await
                .expect("write");
            head
        })
    };
    let http = RelayHttp::new(
        Once::new(RelayStream::from_unix(ours)),
        RelayLimits::default(),
    );
    let url = WebUrl::parse("https://h.example.test/upload/big").expect("url");
    let upload = request(Method::PUT, &url)
        .body(Counting {
            total: TOTAL,
            made: 0,
            produced: Arc::clone(&produced),
            consumed: Arc::clone(&consumed),
            peak: Arc::clone(&peak),
        })
        .expect("request");
    let mut response = http.send_streaming(upload).await.expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let mut body = Vec::new();
    while let Some(piece) = response.body_mut().next().await.expect("piece") {
        body.extend_from_slice(&piece);
    }
    assert_eq!(body, b"ok");

    let head = server.await.expect("server");
    assert!(
        head.to_ascii_lowercase()
            .contains(&format!("content-length: {TOTAL}\r\n")),
        "{head}"
    );
    assert_eq!(produced.load(Ordering::SeqCst), TOTAL);
    assert_eq!(consumed.load(Ordering::SeqCst), TOTAL);
    let peak = peak.load(Ordering::SeqCst);
    assert!(
        peak <= 8 * 1024 * 1024,
        "at most 8 MiB of a 300 MiB body was ever in flight, saw {peak}"
    );
}

#[tokio::test]
async fn a_response_is_read_piece_by_piece_without_a_cap_and_its_connection_is_kept() {
    // 3 MiB against a whole-body cap of 1 KiB: only a streamed read can take it.
    const SIZE: usize = 3 * 1024 * 1024;
    let (ours, mut theirs) = UnixStream::pair().expect("pair");
    let seen = Arc::new(Seen::default());
    let server = {
        let seen = Arc::clone(&seen);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            read_request(&mut theirs, &mut buf, &seen)
                .await
                .expect("request");
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {SIZE}\r\n\r\n");
            theirs.write_all(head.as_bytes()).await.expect("head");
            theirs.write_all(&vec![7u8; SIZE]).await.expect("body");
            // The next request arrives on the same connection: the client kept it.
            read_request(&mut theirs, &mut buf, &seen)
                .await
                .expect("second");
            theirs
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .expect("answer");
        })
    };
    let http = RelayHttp::new(
        Once::new(RelayStream::from_unix(ours)),
        RelayLimits {
            max_body: 1024,
            ..RelayLimits::default()
        },
    );
    let url = WebUrl::parse("https://h.example.test/download").expect("url");
    let get = || {
        request(Method::GET, &url)
            .body(http_body_util::Empty::<Bytes>::new())
            .expect("request")
    };
    let mut response = http.send_streaming(get()).await.expect("response");
    let (mut total, mut pieces) = (0usize, 0usize);
    while let Some(piece) = response.body_mut().next().await.expect("piece") {
        assert!(piece.iter().all(|b| *b == 7));
        total += piece.len();
        pieces += 1;
    }
    assert_eq!(total, SIZE);
    assert!(pieces > 1, "it arrived in pieces, not whole");
    let again = request(Method::GET, &url)
        .body(Vec::new())
        .expect("request");
    let second = http.send(again).await.expect("second answer");
    assert_eq!(second.body(), b"ok");
    server.await.expect("server");
}
