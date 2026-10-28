//! Model turns on an engine of this computer: stoker's `OpenAiCompat` (`Driver<OpenAiCodec,
//! HttpClient>`) against a [`LocalModel`]'s engine, wrapped in `Retrying` (the daemon owns retry,
//! ARCHITECTURE section 7). A [`LocalTurn`] is a [`porter_infer::Model`] made for one request on a
//! local model: chat (validated and repaired when the request's shape asks), tasks and
//! embeddings, its events going to the sink it is given. A [`LocalTurns`] runs the computer-use
//! steps of one session that is pinned to a local model; their events go down a channel of
//! [`TurnStep`]s. The engine's owner is told the engine is in use ([`EngineUse`]) while a turn
//! runs.
//!
//! A reply is never asked to be longer than the ceiling the caller gives a turn (the model's most,
//! the person's length and the engine's window: `LocalModel::reply_ceiling`).
//!
//! What it leaves to its caller: which model the session is pinned to, a hosted model, and a
//! speech turn (a speech engine is not an OpenAI-compatible one).

use crate::bridge::{self, Frames};
use crate::cua_run::{CuaRun, StepJob};
use engine_supervisor::EngineId;
use model_http::{HttpClient, Timeouts, WaitMs as HttpWaitMs};
use model_openai_compat::{Flavor, OpenAiCodec, OpenAiCompat};
use model_provider as sp;
use model_provider::Retrying;
use porter_infer::{
    ChatReply, ChatSink, CuaBegin, CuaStepFailure, CuaStepReply, Flow, InferEvent, InferRefusal,
    InferReply, ModelError, ServedBy,
};
use porter_router::local::LocalModel;
use porter_router::seams::TurnStep;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

mod turn;
pub use turn::LocalTurn;

/// Waits with the clock of the runtime, so a test with paused time does not wait.
#[derive(Debug, Clone, Copy)]
pub struct TokioSleep;

impl sp::Sleeper for TokioSleep {
    fn sleep(&self, wait: sp::WaitMs) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(Duration::from_millis(u64::from(wait.0)))
    }
}

/// Three attempts, a quarter of a second doubling to four seconds (porter-bridge's, shared with
/// inferd-client's in-process engines).
pub const RETRY: sp::RetryPolicy = porter_bridge::ENGINE_RETRY;

/// An engine on a local socket answers its first byte after the prompt is read, which for an
/// image prompt on a cold cache takes a while; between chunks it should not stall.
const TIMEOUTS: Timeouts = Timeouts {
    connect: HttpWaitMs(2_000),
    first_byte: HttpWaitMs(120_000),
    idle: HttpWaitMs(60_000),
};

/// How often a running turn tells the engine's owner its engine is in use.
const TOUCH_EVERY: Duration = Duration::from_millis(250);

/// Told that a turn is using an engine, so it is not idle and never a victim of eviction. The
/// daemon's engine supervisor is the one that matters; an app that starts no engine gives `()`.
pub trait EngineUse: Send + Sync {
    /// A turn is using `engine` now.
    fn used(&self, engine: &EngineId);
}

impl EngineUse for () {
    fn used(&self, _: &EngineId) {}
}

fn client(model: &LocalModel) -> HttpClient {
    // An engine the person attached: its token is read now (so a rotated one is the one sent). A
    // key file that is refused sends no token at all: the engine answers 401.
    HttpClient::new(model.endpoint("/v1", TIMEOUTS))
}

/// The provider of a chat engine on this computer, retried as the daemon retries; none for a model
/// whose engine speaks no chat wire (a speech host).
pub fn chat_provider(model: &LocalModel) -> Option<Retrying<OpenAiCompat, TokioSleep>> {
    let flavor: Flavor = model.flavor?;
    Some(Retrying::new(
        OpenAiCodec::new(flavor).provider(client(model)),
        RETRY,
        TokioSleep,
    ))
}

/// Where a computer-use step's events go: to the session.
#[derive(Debug)]
pub struct ToSession {
    steps: mpsc::UnboundedSender<TurnStep>,
}

impl ToSession {
    /// A sink that sends each event down `steps` as a [`TurnStep::Event`].
    pub fn new(steps: mpsc::UnboundedSender<TurnStep>) -> Self {
        Self { steps }
    }
}

impl ChatSink for ToSession {
    fn event(&mut self, event: InferEvent) -> Flow {
        match self.steps.send(TurnStep::Event(event)) {
            Ok(()) => Flow::Continue,
            Err(_) => Flow::Stop,
        }
    }
}

/// Where a turn's events go: to the session, and into the reply being gathered. `touch` is called
/// no more than every [`TOUCH_EVERY`].
struct Forward<S, T> {
    events: S,
    touch: T,
    last_touch: Instant,
    gathered: bridge::Gathered,
}

impl<S, T> sp::TurnSink for Forward<S, T>
where
    S: FnMut(InferEvent) -> sp::Flow + Send,
    T: FnMut() + Send,
{
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        self.gathered.take(&event);
        if self.last_touch.elapsed() >= TOUCH_EVERY {
            self.last_touch = Instant::now();
            (self.touch)();
        }
        match bridge::event(&event) {
            Some(out) => (self.events)(out),
            None => sp::Flow::Continue,
        }
    }
}

/// Runs `turn` on `provider` as a plain chat turn: each event goes to `events` as it comes (a
/// `Stop` from it ends the turn), `touch` is called while the turn runs (no more than every 250
/// milliseconds; an app that starts no engine passes a closure that does nothing), and the reply
/// is the gathered text, thought and tool calls with the turn's usage. The second value says why
/// a turn that asked for option shares has none, for the caller to note; the reply is the same
/// either way.
pub async fn run_turn<P: sp::Provider>(
    provider: &P,
    turn: &sp::TurnRequest,
    served: &ServedBy,
    events: impl FnMut(InferEvent) -> sp::Flow + Send,
    touch: impl FnMut() + Send,
) -> (Result<ChatReply, ModelError>, Option<bridge::NoScores>) {
    let mut sink = Forward {
        events,
        touch,
        last_touch: Instant::now(),
        gathered: bridge::Gathered::for_turn(turn),
    };
    match provider.turn(turn, &mut sink).await {
        Ok(end) => {
            let (reply, why) = sink.gathered.chat_reply_noted(&end, served.clone());
            (Ok(reply), why)
        }
        Err(error) => (Err(bridge::model_error(&error)), None),
    }
}

/// [`run_turn`] with the reply as the session's [`InferReply`].
pub async fn run_chat<P: sp::Provider>(
    provider: &P,
    turn: &sp::TurnRequest,
    served: &ServedBy,
    events: impl FnMut(InferEvent) -> sp::Flow + Send,
    touch: impl FnMut() + Send,
) -> (InferReply, Option<bridge::NoScores>) {
    let (reply, why) = run_turn(provider, turn, served, events, touch).await;
    (
        match reply {
            Ok(reply) => InferReply::Chat(reply),
            Err(error) => InferReply::Failed(error),
        },
        why,
    )
}

/// Says in the daemon's log why a reply that asked for option shares has none: the reply is the
/// same without them and the app is not told (a Choice's answer never depends on its scores).
pub fn note_no_scores(why: Option<bridge::NoScores>) {
    if let Some(why) = why {
        eprintln!("inferd: choice scores: none: {why}");
    }
}

fn refused(refusal: InferRefusal) -> InferReply {
    InferReply::Refused(refusal)
}

/// The turns of one session on a model of this computer.
#[derive(Debug, Clone)]
pub struct LocalTurns<U> {
    engines: U,
    steps: mpsc::UnboundedSender<TurnStep>,
    run: Arc<Mutex<Option<CuaRun>>>,
}

impl<U: EngineUse> LocalTurns<U> {
    /// Turns that tell `engines` what is in use, send their events down `steps` and keep the
    /// session's computer-use run in `run` (shared by the session's turns, one run at a time).
    pub fn new(
        engines: U,
        steps: mpsc::UnboundedSender<TurnStep>,
        run: Arc<Mutex<Option<CuaRun>>>,
    ) -> Self {
        Self {
            engines,
            steps,
            run,
        }
    }

    /// The owner of the engines.
    pub fn engines(&self) -> &U {
        &self.engines
    }

    /// The channel the events go down.
    pub fn steps(&self) -> &mpsc::UnboundedSender<TurnStep> {
        &self.steps
    }

    /// A `CuaBegin`: the session's run starts afresh with this goal; the reply is an empty step.
    pub fn begin_cua(&self, begin: CuaBegin) -> InferReply {
        *self.run.lock().unwrap_or_else(PoisonError::into_inner) = Some(CuaRun::begin(begin));
        InferReply::CuaStep(CuaStepReply::new(Vec::new()))
    }

    /// One computer-use step of the session's run on `model`; refused `Unsupported` when no run
    /// has begun or the model has no chat wire.
    pub async fn cua(
        &self,
        model: &LocalModel,
        request: &porter_infer::CuaStepRequest,
        frames: &Frames,
        ceiling: sp::Tokens,
    ) -> InferReply {
        let run = self
            .run
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (Some(mut run), Some(provider)) = (run, chat_provider(model)) else {
            return refused(InferRefusal::Unsupported);
        };
        let mut sink = ToSession {
            steps: self.steps.clone(),
        };
        let job = StepJob {
            model,
            provider: &provider,
            frames,
            request,
            ceiling,
        };
        match run.step(job, &mut sink).await {
            Ok(reply) => {
                *self.run.lock().unwrap_or_else(PoisonError::into_inner) = Some(run);
                InferReply::CuaStep(reply)
            }
            Err(CuaStepFailure::Unparseable) => InferReply::Failed(ModelError::Unparseable),
            Err(CuaStepFailure::ModelFailed(error)) => InferReply::Failed(error),
            // a variant a newer porter adds: the step failed as unparseable, no actions
            Err(_) => InferReply::Failed(ModelError::Unparseable),
        }
    }
}
