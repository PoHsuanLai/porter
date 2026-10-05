//! The page the browser is first sent to. A sign-in step names its page as an `EndpointUrl`,
//! which has no query string, and an authorize URL is all query. So the browser opens a tiny
//! listener on this computer that answers one thing: a redirect to the authorize URL. It lives
//! as long as the sign-in does (a "Copy link" opened again still works) and is dropped with it.
//!
//! This stands in for a query-capable URL type on `SignInStep::OpenBrowser` (FINDINGS).

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A running launcher: its address, and the task that serves it (aborted on drop).
#[derive(Debug)]
pub(super) struct Launcher {
    port: u16,
    task: JoinHandle<()>,
}

impl Launcher {
    /// Binds 127.0.0.1 on a free port and serves a `302` to `target` to whoever asks.
    pub async fn start(target: String) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            let answer = redirect(&target);
            while let Ok((mut socket, _)) = listener.accept().await {
                let answer = answer.clone();
                tokio::spawn(async move {
                    // Read the request head (or what a slow client sent in a second), then
                    // answer; nothing in it is used.
                    let mut head = [0u8; 2048];
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        socket.read(&mut head),
                    )
                    .await;
                    let _ = socket.write_all(answer.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Ok(Self { port, task })
    }

    /// The page to open.
    pub fn page(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn redirect(target: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_launcher_redirects_every_visit_to_the_authorize_url() {
        let launcher = Launcher::start("https://login.example/authorize?a=1&b=2".into())
            .await
            .expect("bind");
        for _ in 0..2 {
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", launcher.port))
                .await
                .expect("connect");
            socket
                .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .expect("send");
            let mut reply = String::new();
            socket.read_to_string(&mut reply).await.expect("read");
            assert!(reply.starts_with("HTTP/1.1 302"), "{reply}");
            assert!(reply.contains("Location: https://login.example/authorize?a=1&b=2\r\n"));
        }
    }
}
