//! A way for a program that can only dial a local path to reach one of the person's computers.
//!
//! A [`Relay`] listens on a unix socket at a path of its own and, for each connection to it,
//! asks the [`Dialer`] for a connection to the computer (which asks Tailscale who is at the
//! address, every time) and then carries the bytes both ways. It reads none of them. When the
//! computer cannot be reached, the program that dialed is answered with a short refusal in the
//! words of the reason, so that what it shows says why.

use crate::dial::{DialError, Dialer};
use porter_core::NodeId;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, copy_bidirectional};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::{JoinHandle, JoinSet};

/// Told of each connection that could not be carried to the computer.
pub type OnFault = Arc<dyn Fn(&DialError) + Send + Sync>;

/// A listener at a local path that leads to one computer; it stops when dropped (the path stays,
/// and the next relay at it replaces the file).
#[derive(Debug)]
pub struct Relay {
    task: JoinHandle<()>,
}

impl Relay {
    /// Listens at `path` and carries each connection to the computer `node`.
    pub fn start(
        path: &Path,
        node: NodeId,
        dialer: Dialer,
        on_fault: OnFault,
    ) -> std::io::Result<Self> {
        // A file left by an earlier run is not a listener any more.
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(path)?;
        let task = tokio::spawn(serve(listener, node, dialer, on_fault));
        Ok(Self { task })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(listener: UnixListener, node: NodeId, dialer: Dialer, on_fault: OnFault) {
    let mut carrying = JoinSet::new();
    let mut pause = std::time::Duration::from_millis(50);
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                pause = std::time::Duration::from_millis(50);
                carrying.spawn(carry(
                    stream,
                    node.clone(),
                    dialer.clone(),
                    Arc::clone(&on_fault),
                ));
                while carrying.try_join_next().is_some() {}
            }
            Err(_) => {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(std::time::Duration::from_secs(2));
            }
        }
    }
}

async fn carry(mut local: UnixStream, node: NodeId, dialer: Dialer, on_fault: OnFault) {
    match dialer.connect(&node).await {
        Ok(mut remote) => {
            let _ = copy_bidirectional(&mut local, &mut remote).await;
        }
        Err(fault) => {
            on_fault(&fault);
            let body = serde_json::json!({
                "error": {"message": fault.to_string(), "type": "unreachable", "code": "unreachable"}
            })
            .to_string();
            let answer = format!(
                "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = local.write_all(answer.as_bytes()).await;
            let _ = local.shutdown().await;
        }
    }
}
