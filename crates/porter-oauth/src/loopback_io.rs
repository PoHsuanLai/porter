//! The loopback listener (feature `io`), ported from mailo's
//! (`~/mailo/crates/mail-runtime/src/loopback.rs`): bound to 127.0.0.1 only, never the wildcard,
//! which would accept an authorization code from anywhere on the network.

use crate::loopback::{
    AuthCode, LoopbackFault, MAX_REDIRECT_BYTES, REDIRECT_WAIT_SECONDS, parse_redirect,
};
use crate::pkce::OAuthState;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A listener on 127.0.0.1 at a port the OS chose.
#[derive(Debug)]
pub struct LoopbackServer {
    listener: TcpListener,
    port: u16,
}

impl LoopbackServer {
    /// Binds 127.0.0.1 on a free port.
    pub async fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// The port, for the redirect URI.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The redirect URI to send with the authorize request. It has a path (`/`), so the issuer
    /// appends `?code=` to a URL with an explicit target.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Accepts one request within the wait limit, answers the person with a page, and returns
    /// the code if the state is `expected`. The listener is consumed: whatever arrives second
    /// finds nothing listening.
    pub async fn wait(self, expected: &OAuthState) -> Result<AuthCode, LoopbackFault> {
        self.wait_for(expected, Duration::from_secs(REDIRECT_WAIT_SECONDS))
            .await
    }

    async fn wait_for(
        self,
        expected: &OAuthState,
        limit: Duration,
    ) -> Result<AuthCode, LoopbackFault> {
        tokio::time::timeout(limit, self.accept_one(expected))
            .await
            .unwrap_or(Err(LoopbackFault::TimedOut))
    }

    async fn accept_one(self, expected: &OAuthState) -> Result<AuthCode, LoopbackFault> {
        let (mut socket, _) = self
            .listener
            .accept()
            .await
            .map_err(|_| LoopbackFault::Malformed)?;
        drop(self.listener);
        let outcome = match read_head(&mut socket).await {
            Ok(head) => parse_redirect(&head, expected),
            Err(fault) => Err(fault),
        };
        respond(&mut socket, page(&outcome)).await;
        outcome
    }
}

/// Reads through the blank line, and no further than the cap.
async fn read_head(socket: &mut TcpStream) -> Result<Vec<u8>, LoopbackFault> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > MAX_REDIRECT_BYTES {
            return Err(LoopbackFault::Oversized);
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return Err(LoopbackFault::Malformed),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    Ok(buf)
}

/// What the person reads. Fixed text: nothing from the request is reflected, so a crafted URL
/// cannot put content in their browser.
fn page(outcome: &Result<AuthCode, LoopbackFault>) -> &'static str {
    match outcome {
        Ok(_) => "Signed in. You can close this tab.",
        Err(LoopbackFault::Refused(_)) => "Authorization was declined.",
        Err(_) => "Not what this port is for.",
    }
}

async fn respond(socket: &mut TcpStream, message: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><p>{message}");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> OAuthState {
        OAuthState("st-1".into())
    }

    async fn send_raw(port: u16, bytes: Vec<u8>) -> String {
        let mut socket = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        let _ = socket.write_all(&bytes).await;
        let mut answer = Vec::new();
        let _ = socket.read_to_end(&mut answer).await;
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[tokio::test]
    async fn it_binds_loopback_only_and_names_its_redirect() {
        let server = LoopbackServer::bind().await.expect("bind");
        assert_eq!(
            server.listener.local_addr().expect("addr").ip().to_string(),
            "127.0.0.1"
        );
        assert_eq!(
            server.redirect_uri(),
            format!("http://127.0.0.1:{}/", server.port())
        );
    }

    #[tokio::test]
    async fn a_good_redirect_yields_its_code_and_a_page() {
        let server = LoopbackServer::bind().await.expect("bind");
        let port = server.port();
        let client = tokio::spawn(send_raw(
            port,
            b"GET /?code=abc&state=st-1 HTTP/1.1\r\n\r\n".to_vec(),
        ));
        let code = server.wait(&state()).await.expect("code");
        assert_eq!(code.0.expose(), "abc");
        assert!(client.await.expect("client").contains("Signed in"));
    }

    #[tokio::test]
    async fn the_wrong_state_ends_the_wait_and_a_second_request_finds_no_listener() {
        let server = LoopbackServer::bind().await.expect("bind");
        let port = server.port();
        let client = tokio::spawn(send_raw(
            port,
            b"GET /?code=stolen&state=wrong HTTP/1.1\r\n\r\n".to_vec(),
        ));
        assert_eq!(server.wait(&state()).await, Err(LoopbackFault::WrongState));
        let page = client.await.expect("client");
        assert!(!page.contains("stolen"));
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_err());
    }

    #[tokio::test]
    async fn an_oversized_request_is_refused_without_reading_it_all() {
        let server = LoopbackServer::bind().await.expect("bind");
        let port = server.port();
        let huge = format!("GET /?code={} HTTP/1.1\r\n\r\n", "a".repeat(64 * 1024));
        let client = tokio::spawn(send_raw(port, huge.into_bytes()));
        assert_eq!(server.wait(&state()).await, Err(LoopbackFault::Oversized));
        let _ = client.await;
    }

    #[tokio::test]
    async fn a_silent_browser_times_out() {
        let server = LoopbackServer::bind().await.expect("bind");
        let outcome = server.wait_for(&state(), Duration::from_millis(50)).await;
        assert_eq!(outcome, Err(LoopbackFault::TimedOut));
    }
}
