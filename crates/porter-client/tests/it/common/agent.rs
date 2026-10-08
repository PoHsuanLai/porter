//! A hand-written latchkey agent on a Unix socket in a scratch directory: the other end of
//! `SocketTransport`. It reads the first frame exactly, then either answers one accountd call
//! with the real `AccountService`, or serves a session with the real session server over scripted
//! seams. Every hello it was sent is kept.

use super::served::{Gate, Route, Scripted, Unaudited};
use inferd::serve::{Seams, serve_session};
use inferd::session::SessionSpec;
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_core::{AccountsReply, AccountsRequest, AppId};
use porter_fake::FakeService;
use porter_infer::LinkHello;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::io::IoSlice;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::Interest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// The agent's name under its scratch runtime directory (short: the socket path has 108 bytes).
pub const AGENT_NAME: &str = "pt";

/// Where latchkey puts the agent called `name` under the runtime directory `dir`.
pub fn socket_in(dir: &std::path::Path, name: &str) -> PathBuf {
    let env = latchkey::Environment {
        runtime_dir: Some(dir.as_os_str()),
        tmpdir: Some(dir.as_os_str()),
        ..latchkey::Environment::default()
    };
    latchkey::Agent::in_environment(name, latchkey::here(), &env)
        .expect("an address")
        .socket()
        .expect("a socket")
        .to_path_buf()
}

/// A running agent.
#[derive(Debug)]
pub struct Agent {
    /// How a client names it: latchkey's agent under the scratch runtime directory.
    pub door: porter_client::SocketAgent,
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
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "a-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
            usage: porter_core::consent::Usage::Interactive,
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
    if let AccountsRequest::OpenAuthenticated { grant, endpoint } = &envelope.body {
        return open_authenticated(stream, &plan, grant, endpoint).await;
    }
    let reply = plan.service.handle(&plan.app, envelope.body).await;
    if let Ok(frame) = encode_frame(&reply) {
        let _ = stream.write_all(&frame).await;
    }
}

/// Starts an agent on a new socket.
pub fn start(name: &str, plan: Plan) -> Agent {
    let scratch = scratch(name);
    let path = socket_in(&scratch, AGENT_NAME);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("agent dir");
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
        door: porter_client::SocketAgent::named(AGENT_NAME).in_runtime_dir(scratch.clone()),
        path,
        hellos,
        runner,
        scratch,
        task,
    }
}

/// `OpenAuthenticated`: the service plans the relay (or refuses); the agent keeps one end of a
/// socketpair, writes the planned endpoint on it where a relay would run, and sends the other
/// end with the reply as `SCM_RIGHTS`.
async fn open_authenticated(
    mut stream: UnixStream,
    plan: &Plan,
    grant: &porter_core::GrantId,
    endpoint: &porter_core::EndpointUrl,
) {
    let relay_plan = match plan
        .service
        .open_authenticated(&plan.app, grant, endpoint)
        .await
    {
        Ok(relay_plan) => relay_plan,
        Err(refusal) => {
            if let Ok(frame) = encode_frame(&AccountsReply::Refused(refusal)) {
                let _ = stream.write_all(&frame).await;
            }
            return;
        }
    };
    let Ok((app_end, relay_end)) = std::os::unix::net::UnixStream::pair() else {
        return;
    };
    relay_end.set_nonblocking(true).expect("nonblocking");
    let mut relay_end = UnixStream::from_std(relay_end).expect("tokio stream");
    let Ok(frame) = encode_frame(&AccountsReply::Authenticated) else {
        return;
    };
    let fd: OwnedFd = app_end.into();
    let sent = stream
        .async_io(Interest::WRITABLE, || {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut control = SendAncillaryBuffer::new(&mut space);
            let borrowed = [fd.as_fd()];
            control.push(SendAncillaryMessage::ScmRights(&borrowed));
            sendmsg(
                &stream,
                &[IoSlice::new(&frame)],
                &mut control,
                SendFlags::NOSIGNAL,
            )
            .map_err(std::io::Error::from)
        })
        .await;
    if sent.is_ok() {
        let _ = relay_end
            .write_all(format!("relay for {}\r\n", relay_plan.endpoint.url).as_bytes())
            .await;
        // The relay lives as long as the app's end.
        let mut sink = [0u8; 64];
        while relay_end.read(&mut sink).await.is_ok_and(|n| n > 0) {}
    }
}
