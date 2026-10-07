//! Scripted seams and a raw client for the session server's tests. The test plays the engine: a
//! started turn hands the test a sender, and the turn yields whatever the test pushes.

use inferd::serve::{
    AuditSink, Carried, EngineFailed, EngineHost, Router, RunningTurn, TurnRunner, TurnStep,
};
use inferd::session::{RouteDecision, SessionSpec};
use porter_core::wire::{FrameRead, decode_frame, encode_frame};
use porter_core::{AccountId, DataClass, Locality, ModelId, Need, Tier, Tokens};
use porter_infer::{
    AudioFrame, ClientFrame, InferEvent, InferRefusal, InferReply, InferRequest, ModelRef,
    Readiness, ServedBy,
};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use std::io::{IoSlice, Read, Seek};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, Interest};
use tokio::net::UnixStream;
use tokio::sync::{Notify, mpsc};

pub fn served() -> ServedBy {
    ServedBy {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("qwen").expect("id"),
        locality: Locality::OnDevice,
    }
}

pub fn model() -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse("qwen").expect("id"),
    }
}

pub fn spec(need: Need, class: DataClass) -> SessionSpec {
    SessionSpec {
        need,
        class,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

pub fn llm() -> Need {
    Need::Llm(porter_core::need::LlmNeed {
        features: Default::default(),
        context: Tokens(1),
    })
}

/// A router with one fixed answer.
#[derive(Debug)]
pub struct FixedRouter(pub Result<RouteDecision, InferRefusal>);

impl FixedRouter {
    pub fn ready() -> Self {
        Self(Ok(RouteDecision {
            served: served(),
            readiness: Readiness::Ready,
        }))
    }

    pub fn loading() -> Self {
        Self(Ok(RouteDecision {
            served: served(),
            readiness: Readiness::Loadable,
        }))
    }
}

impl Router for FixedRouter {
    async fn route(&self, _spec: &SessionSpec) -> Result<RouteDecision, InferRefusal> {
        self.0.clone()
    }
}

/// Engines the test starts by hand: `want` resolves when `go` is notified, or at once.
#[derive(Debug, Default)]
pub struct Engines {
    pub go: Option<Arc<Notify>>,
    pub fail: bool,
    pub released: Arc<Mutex<Vec<ModelRef>>>,
}

impl EngineHost for Engines {
    async fn want(&self, _model: ModelRef) -> Result<(), EngineFailed> {
        if let Some(go) = &self.go {
            go.notified().await;
        }
        match self.fail {
            true => Err(EngineFailed::unknown()),
            false => Ok(()),
        }
    }

    fn release(&self, model: &ModelRef) {
        self.released.lock().expect("lock").push(model.clone());
    }
}

/// What a started turn gave the test.
#[derive(Debug)]
pub struct Started {
    pub request: InferRequest,
    /// The bytes of each descriptor that rode with the request.
    pub attachments: Vec<Vec<u8>>,
    /// Push what the engine says.
    pub tx: mpsc::UnboundedSender<TurnStep>,
}

#[derive(Debug, Default, Clone)]
pub struct Runner {
    pub started: Arc<Mutex<Vec<Started>>>,
    pub audio: Arc<Mutex<Vec<AudioFrame>>>,
    pub audio_ended: Arc<Mutex<usize>>,
}

#[derive(Debug)]
pub struct Turn {
    rx: mpsc::UnboundedReceiver<TurnStep>,
    runner: Runner,
}

impl TurnRunner for Runner {
    type Turn = Turn;

    fn start(&self, request: InferRequest, attachments: Vec<OwnedFd>) -> Turn {
        let (tx, rx) = mpsc::unbounded_channel();
        let contents = attachments
            .into_iter()
            .map(|fd| {
                let mut file = std::fs::File::from(fd);
                let mut bytes = Vec::new();
                file.rewind().expect("rewind");
                file.read_to_end(&mut bytes).expect("read");
                bytes
            })
            .collect();
        self.started.lock().expect("lock").push(Started {
            request,
            attachments: contents,
            tx,
        });
        Turn {
            rx,
            runner: self.clone(),
        }
    }

    fn start_heard(
        &self,
        chat: porter_infer::ChatRequest,
        heard: inferd::session::HeardAudio,
    ) -> Turn {
        self.audio.lock().expect("lock").extend(heard.frames);
        self.start(InferRequest::Chat(chat), Vec::new())
    }
}

impl RunningTurn for Turn {
    fn audio(&mut self, frame: AudioFrame) {
        self.runner.audio.lock().expect("lock").push(frame);
    }

    fn end_audio(&mut self) {
        *self.runner.audio_ended.lock().expect("lock") += 1;
    }

    async fn next(&mut self) -> TurnStep {
        match self.rx.recv().await {
            Some(step) => step,
            None => std::future::pending().await,
        }
    }
}

/// The replies the audit sink was told of, and what each turn's request carried.
#[derive(Debug, Default, Clone)]
pub struct Audit(
    pub Arc<Mutex<Vec<InferReply>>>,
    pub Arc<Mutex<Vec<Carried>>>,
);

impl AuditSink for Audit {
    fn record(
        &self,
        _spec: &SessionSpec,
        served_by: &ServedBy,
        reply: &InferReply,
        carried: &Carried,
    ) {
        assert_eq!(served_by, &served());
        self.0.lock().expect("lock").push(reply.clone());
        self.1.lock().expect("lock").push(*carried);
    }
}

/// The client's end of a session socket.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    inbox: Vec<u8>,
}

impl Client {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            inbox: Vec::new(),
        }
    }

    /// Writes a frame with descriptors on it (one `sendmsg`, as the real client does).
    pub async fn send(&mut self, frame: &ClientFrame, fds: &[OwnedFd]) {
        let bytes = encode_frame(frame).expect("encode");
        let sent = self
            .stream
            .async_io(Interest::WRITABLE, || {
                let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(16))];
                let mut control = SendAncillaryBuffer::new(&mut space);
                let borrowed: Vec<_> = fds.iter().map(AsFd::as_fd).collect();
                if !borrowed.is_empty() {
                    control.push(SendAncillaryMessage::ScmRights(&borrowed));
                }
                sendmsg(
                    &self.stream,
                    &[IoSlice::new(&bytes)],
                    &mut control,
                    SendFlags::NOSIGNAL,
                )
                .map_err(std::io::Error::from)
            })
            .await
            .expect("sendmsg");
        assert_eq!(sent, bytes.len(), "the test frames are small");
    }

    /// The next event, or `None` when the daemon closed the socket.
    pub async fn event(&mut self) -> Option<InferEvent> {
        tokio::time::timeout(Duration::from_secs(5), self.event_inner())
            .await
            .expect("an event or a close within five seconds")
    }

    async fn event_inner(&mut self) -> Option<InferEvent> {
        loop {
            if let Ok(FrameRead::Complete(envelope, used)) = decode_frame::<InferEvent>(&self.inbox)
            {
                self.inbox.drain(..used);
                return Some(envelope.body);
            }
            let mut chunk = [0_u8; 4096];
            match self.stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => self.inbox.extend_from_slice(&chunk[..n]),
            }
        }
    }

    /// Events up to and including the next `Finished`.
    pub async fn until_finished(&mut self) -> Vec<InferEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.event().await {
            let done = matches!(event, InferEvent::Finished(_));
            events.push(event);
            if done {
                break;
            }
        }
        events
    }
}

/// Waits (briefly) until `check` holds; panics after five seconds.
pub async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}
