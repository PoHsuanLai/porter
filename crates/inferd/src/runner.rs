//! The model turns of one session: stoker's `OpenAiCompat` (`Driver<OpenAiCodec, HttpClient>`)
//! against the pinned model's engine socket, wrapped in `Retrying` (inferd owns retry,
//! ARCHITECTURE section 7). A turn is a task that pushes `TurnStep`s down a channel; dropping the
//! turn aborts the task, which drops the HTTP future and closes the engine's stream.
//!
//! What a session is pinned to is decided by its router and read here through a [`Pin`] cell:
//! `serve_session` calls the router once and the runner never sees the decision otherwise.
//! Speech turns are refused `Unsupported` (the speech runner is not built).

use crate::bridge::{self, Frames};
use crate::cua_step::{self, Run};
use crate::hosts::unix_endpoint;
use crate::local::LocalModel;
use crate::serve::{RunningTurn, TurnRunner, TurnStep};
use crate::supervise::Supervised;
use model_http::{HttpClient, Timeouts, WaitMs as HttpWaitMs};
use model_openai_compat::{Flavor, OpenAiCodec, OpenAiCompat};
use model_provider as sp;
use model_provider::{Embedder, Provider, Retrying};
use porter_core::Tier;
use porter_infer::{
    CuaStepFailure, CuaStepReply, EmbedReply, InferEvent, InferRefusal, InferReply,
    InferRequest, ModelError, ServedBy, TokenUsage,
};
use std::future::Future;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// What the router decided for one session.
#[derive(Debug, Clone)]
pub struct Pinned {
    /// The account, model and locality (what the client is told and the audit records).
    pub served: ServedBy,
    /// The local model behind it; none for a model that is not served from this computer.
    pub model: Option<Arc<LocalModel>>,
}

/// The cell the router writes and the runner reads.
#[derive(Debug, Clone, Default)]
pub struct Pin(Arc<OnceLock<Pinned>>);

impl Pin {
    /// An empty cell.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the decision (the first one stands).
    pub fn set(&self, pinned: Pinned) {
        let _ = self.0.set(pinned);
    }

    fn get(&self) -> Option<&Pinned> {
        self.0.get()
    }
}

/// Waits with the clock of the runtime, so a test with paused time does not wait.
#[derive(Debug, Clone, Copy)]
pub struct TokioSleep;

impl sp::Sleeper for TokioSleep {
    fn sleep(&self, wait: sp::WaitMs) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(Duration::from_millis(u64::from(wait.0)))
    }
}

/// Three attempts, a quarter of a second doubling to four seconds.
const RETRY: sp::RetryPolicy = sp::RetryPolicy {
    attempts: sp::Attempt(3),
    base: sp::WaitMs(250),
    cap: sp::WaitMs(4000),
};

/// An engine on a local socket answers its first byte after the prompt is read, which for an
/// image prompt on a cold cache takes a while; between chunks it should not stall.
const TIMEOUTS: Timeouts = Timeouts {
    connect: HttpWaitMs(2_000),
    first_byte: HttpWaitMs(120_000),
    idle: HttpWaitMs(60_000),
};

/// How often a running turn tells the supervisor its engine is in use.
const TOUCH_EVERY: Duration = Duration::from_millis(250);

fn client(model: &LocalModel) -> HttpClient {
    HttpClient::new(unix_endpoint(model.socket.0.clone(), "/v1", TIMEOUTS))
}

fn chat_provider(model: &LocalModel) -> Option<Retrying<OpenAiCompat, TokioSleep>> {
    let flavor: Flavor = model.flavor?;
    Some(Retrying::new(
        OpenAiCodec::new(flavor).provider(client(model)),
        RETRY,
        TokioSleep,
    ))
}

/// Starts the turns of one session.
#[derive(Debug, Clone)]
pub struct Turns {
    pin: Pin,
    engines: Supervised,
    tier: Tier,
    run: Arc<Mutex<Option<Run>>>,
}

impl Turns {
    /// Turns for a session of this tier, over the cell its router writes.
    pub fn new(pin: Pin, engines: Supervised, tier: Tier) -> Self {
        Self {
            pin,
            engines,
            tier,
            run: Arc::default(),
        }
    }
}

/// A turn in flight.
#[derive(Debug)]
pub struct Turn {
    steps: mpsc::UnboundedReceiver<TurnStep>,
    task: JoinHandle<()>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RunningTurn for Turn {
    fn audio(&mut self, _: porter_infer::AudioFrame) {}

    fn end_audio(&mut self) {}

    async fn next(&mut self) -> TurnStep {
        match self.steps.recv().await {
            Some(step) => step,
            // The task ended without an answer: it was aborted or it panicked.
            None => TurnStep::Done(InferReply::Failed(ModelError::Unreachable)),
        }
    }
}

impl TurnRunner for Turns {
    type Turn = Turn;

    fn start(&self, request: InferRequest, attachments: Vec<OwnedFd>) -> Turn {
        let (steps, inbox) = mpsc::unbounded_channel();
        let job = Job {
            pinned: self.pin.get().cloned(),
            engines: self.engines.clone(),
            tier: self.tier,
            run: Arc::clone(&self.run),
            steps,
        };
        let task = tokio::spawn(async move {
            let reply = job.reply(request, attachments).await;
            let _ = job.steps.send(TurnStep::Done(reply));
        });
        Turn {
            steps: inbox,
            task,
        }
    }
}

struct Job {
    pinned: Option<Pinned>,
    engines: Supervised,
    tier: Tier,
    run: Arc<Mutex<Option<Run>>>,
    steps: mpsc::UnboundedSender<TurnStep>,
}

/// Where a turn's events go: to the session, and into the reply being gathered.
struct Forward {
    steps: mpsc::UnboundedSender<TurnStep>,
    engines: Supervised,
    engine: engine_supervisor::EngineId,
    last_touch: Instant,
    gathered: bridge::Gathered,
}

impl sp::TurnSink for Forward {
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        self.gathered.take(&event);
        if self.last_touch.elapsed() >= TOUCH_EVERY {
            self.last_touch = Instant::now();
            self.engines.used(&self.engine);
        }
        match bridge::event(&event) {
            Some(out) => send(&self.steps, out),
            None => sp::Flow::Continue,
        }
    }
}

fn send(steps: &mpsc::UnboundedSender<TurnStep>, event: InferEvent) -> sp::Flow {
    match steps.send(TurnStep::Event(event)) {
        Ok(()) => sp::Flow::Continue,
        Err(_) => sp::Flow::Stop,
    }
}

fn refused(refusal: InferRefusal) -> InferReply {
    InferReply::Refused(refusal)
}

impl Job {
    async fn reply(&self, request: InferRequest, attachments: Vec<OwnedFd>) -> InferReply {
        let Some(Pinned {
            served,
            model: Some(model),
        }) = &self.pinned
        else {
            return InferReply::Failed(ModelError::Unreachable);
        };
        self.engines.used(&model.spec.id);
        let frames = match Frames::read(attachments) {
            Ok(frames) => frames,
            Err(_) => return InferReply::Failed(ModelError::Unreadable),
        };
        match request {
            InferRequest::Chat(chat) => match bridge::chat_turn(model, &chat, &frames) {
                Ok(turn) => self.chat(model, served, &turn).await,
                Err(_) => refused(InferRefusal::Unsupported),
            },
            InferRequest::Task(task) => match bridge::task_turn(model, &task, self.tier) {
                Ok(turn) => self.chat(model, served, &turn).await,
                Err(_) => refused(InferRefusal::Unsupported),
            },
            InferRequest::Embed(embed) => self.embed(model, served, &embed).await,
            InferRequest::CuaBegin(begin) => {
                *self.run.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(Run::new(begin));
                InferReply::CuaStep(CuaStepReply {
                    thought: None,
                    actions: Vec::new(),
                    dropped: Vec::new(),
                    safety: Vec::new(),
                })
            }
            InferRequest::CuaStep(step) => self.cua(model, &step, &frames).await,
            InferRequest::Transcribe(_) | InferRequest::Speak(_) => {
                refused(InferRefusal::Unsupported)
            }
        }
    }

    async fn chat(&self, model: &LocalModel, served: &ServedBy, turn: &sp::TurnRequest) -> InferReply {
        let Some(provider) = chat_provider(model) else {
            return refused(InferRefusal::Unsupported);
        };
        let mut sink = Forward {
            steps: self.steps.clone(),
            engines: self.engines.clone(),
            engine: model.spec.id.clone(),
            last_touch: Instant::now(),
            gathered: bridge::Gathered::default(),
        };
        match provider.turn(turn, &mut sink).await {
            Ok(end) => InferReply::Chat(sink.gathered.chat_reply(&end, served.clone())),
            Err(error) => InferReply::Failed(bridge::model_error(&error)),
        }
    }

    async fn embed(
        &self,
        model: &LocalModel,
        served: &ServedBy,
        request: &porter_infer::EmbedRequest,
    ) -> InferReply {
        let Some(flavor) = model.flavor else {
            return refused(InferRefusal::Unsupported);
        };
        let Ok(turns) = bridge::embed_turns(model, request) else {
            return refused(InferRefusal::Unsupported);
        };
        let provider = OpenAiCodec::new(flavor).provider(client(model));
        let mut vectors = Vec::new();
        let mut input = sp::Tokens(0);
        for turn in &turns {
            match provider.embed(turn).await {
                Ok(end) => {
                    input = sp::Tokens(input.0.saturating_add(end.usage.input.0));
                    vectors.extend(bridge::vectors(end.vectors));
                }
                Err(error) => return InferReply::Failed(bridge::model_error(&error)),
            }
            self.engines.used(&model.spec.id);
        }
        InferReply::Embed(EmbedReply {
            vectors,
            usage: TokenUsage {
                input: porter_core::Tokens(input.0),
                output: porter_core::Tokens(0),
                cached: porter_core::Tokens(0),
            },
            served: served.clone(),
        })
    }

    async fn cua(
        &self,
        model: &LocalModel,
        request: &porter_infer::CuaStepRequest,
        frames: &Frames,
    ) -> InferReply {
        let run = self
            .run
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (Some(run), Some(provider)) = (run, chat_provider(model)) else {
            return refused(InferRefusal::Unsupported);
        };
        let steps = self.steps.clone();
        let forward = move |event| send(&steps, event);
        match cua_step::step(&run, model, &provider, request, frames, forward).await {
            Ok((reply, next)) => {
                *self.run.lock().unwrap_or_else(PoisonError::into_inner) = Some(next);
                InferReply::CuaStep(reply)
            }
            Err(CuaStepFailure::Unparseable) => InferReply::Failed(ModelError::Unparseable),
            Err(CuaStepFailure::ModelFailed(error)) => InferReply::Failed(error),
        }
    }
}

#[cfg(test)]
mod tests;
