//! A scripted HTTP/1.1 peer for the other modules: it reads each request (its head and its
//! `Content-Length` body, counting the body's bytes) and writes the next answer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// What a peer has read so far.
#[derive(Debug, Default)]
pub struct Seen {
    /// The heads of the requests, in order.
    pub heads: std::sync::Mutex<Vec<String>>,
    /// The body bytes read, over all requests.
    pub body_bytes: Arc<AtomicU64>,
}

impl Seen {
    pub fn heads(&self) -> Vec<String> {
        self.heads.lock().expect("lock").clone()
    }
}

/// Reads one request off `stream` (`None` when the stream ended first), noting it in `seen`.
pub async fn read_request<S: AsyncRead + Unpin>(
    stream: &mut S,
    buf: &mut Vec<u8>,
    seen: &Seen,
) -> Option<String> {
    let end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let mut chunk = [0u8; 8192];
        match stream.read(&mut chunk).await {
            Ok(n) if n > 0 => buf.extend_from_slice(&chunk[..n]),
            _ => return None,
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    buf.drain(..end);
    let wanted: u64 = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut left = wanted;
    let taken = u64::try_from(buf.len()).unwrap_or(u64::MAX).min(left);
    buf.drain(..usize::try_from(taken).unwrap_or(0));
    seen.body_bytes.fetch_add(taken, Ordering::SeqCst);
    left -= taken;
    let mut chunk = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(chunk.len());
        match stream.read(&mut chunk[..want]).await {
            Ok(n) if n > 0 => {
                seen.body_bytes.fetch_add(n as u64, Ordering::SeqCst);
                left -= n as u64;
            }
            _ => return None,
        }
    }
    seen.heads.lock().expect("lock").push(head.clone());
    Some(head)
}

/// What a peer does once its answers are given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// Closes the connection.
    Close,
    /// Goes quiet with the connection open.
    Quiet,
}

/// Answers each request on `stream` with the next of `answers`, then does `after`.
pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    answers: Vec<String>,
    after: After,
    seen: Arc<Seen>,
) {
    let mut buf = Vec::new();
    for answer in answers {
        if read_request(&mut stream, &mut buf, &seen).await.is_none() {
            return;
        }
        if stream.write_all(answer.as_bytes()).await.is_err() {
            return;
        }
    }
    if after == After::Quiet {
        std::future::pending::<()>().await;
    }
}
