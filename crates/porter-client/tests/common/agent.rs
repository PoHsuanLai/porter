//! A hand-written latchkey agent on a Unix socket in a scratch directory: the other end of
//! `SocketTransport`. It reads the first frame exactly, then either answers one accountd call
//! with the real `AccountService`, or serves a session with the real session server over scripted
//! seams. Every hello it was sent is kept.

use super::served::{Gate, Route, Scripted, Unaudited};
use inferd::serve::{Seams, serve_session};
use inferd::session::SessionSpec;
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_core::{AccountsRequest, AppId};
use porter_fake::FakeService;
use porter_infer::LinkHello;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// A running agent.
#[derive(Debug)]
pub struct Agent {
    pub path: PathBuf,
    /// Every hello it was sent, in order.
    pub hellos: Arc<Mutex<Vec<LinkHello>>>,
    /// What the scripted engine was given.
    pub runner: Scripted,
    scratch: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// How the agent answers.
#[derive(Debug, Clone)]
pub struct Plan {
    pub service: Arc<FakeService>,
    pub app: AppId,
    pub route: Route,
    pub gate: Gate,
    pub runner: Scripted,
}

/// A directory of this test's own.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "agent-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Reads one whole frame's bytes and no more: whatever follows belongs to the session.
async fn first_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await.ok()?;
    let len = u32::from_be_bytes(head) as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.ok()?;
    Some(head.into_iter().chain(body).collect())
}

async fn handle(mut stream: UnixStream, plan: Plan, hellos: Arc<Mutex<Vec<LinkHello>>>) {
    let Some(bytes) = first_frame(&mut stream).await else {
        return;
    };
    if let Ok(FrameRead::Complete(envelope, _)) = decode_frame::<LinkHello>(&bytes) {
        let LinkHello::Open(open) = envelope.body.clone();
        hellos.lock().expect("lock").push(envelope.body);
        let spec = SessionSpec {
            need: open.need,
            class: open.class,
            tier: open.tier,
        };
        let seams = Seams {
            router: plan.route,
            engines: plan.gate,
            runner: plan.runner,
            audit: Unaudited,
        };
        serve_session(stream, spec, &seams).await;
        return;
    }
    let Ok(FrameRead::Complete(envelope, _)) = decode_frame::<AccountsRequest>(&bytes) else {
        return;
    };
    let reply = plan.service.handle(&plan.app, envelope.body).await;
    if let Ok(frame) = encode_frame(&reply) {
        let _ = stream.write_all(&frame).await;
    }
}

/// Starts an agent on a new socket.
pub fn start(name: &str, plan: Plan) -> Agent {
    let scratch = scratch(name);
    let path = scratch.join("agent.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let hellos = Arc::new(Mutex::new(Vec::new()));
    let runner = plan.runner.clone();
    let task = tokio::spawn({
        let hellos = Arc::clone(&hellos);
        async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(handle(stream, plan.clone(), Arc::clone(&hellos)));
            }
        }
    });
    Agent {
        path,
        hellos,
        runner,
        scratch,
        task,
    }
}
