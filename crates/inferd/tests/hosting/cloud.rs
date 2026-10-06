//! A fake hosted-model API: TLS on loopback with the scratch CA's leaf for `localhost`, serving
//! the same scripted chat completions as the fake engine (it is the same server, over TLS). It
//! keeps every request it was sent, headers included, so a test can see the `Authorization` a
//! provider would see.

use super::engine::{Chat, Script, Seen, State, serve};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A running fake provider.
#[derive(Debug)]
pub struct FakeCloud {
    pub seen: Arc<Mutex<Vec<Seen>>>,
    pub port: u16,
    task: JoinHandle<()>,
}

impl Drop for FakeCloud {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeCloud {
    /// Listens on a loopback port, answering as `script` says.
    pub async fn start(script: Script) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("address").port();
        let acceptor = porter_fake_servers::tls::acceptor();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state: State = Arc::new(Mutex::new((VecDeque::from(script.chat), script.dims)));
        let log = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let (acceptor, state, log) =
                    (acceptor.clone(), Arc::clone(&state), Arc::clone(&log));
                tokio::spawn(async move {
                    // A client that does not trust the leaf ends the handshake: nothing is read.
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        serve(tls, state, log).await;
                    }
                });
            }
        });
        Self { seen, port, task }
    }

    /// The requests received, oldest first.
    pub fn requests(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }
}

/// A script of one scripted answer, repeated.
pub fn says(pieces: Vec<&'static str>) -> Script {
    Script {
        chat: vec![Chat::Say(pieces)],
        dims: 0,
    }
}
