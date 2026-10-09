//! Model turns on an engine of this computer: stoker's `OpenAiCompat` (`Driver<OpenAiCodec,
//! HttpClient>`) against a [`LocalModel`]'s engine, wrapped in `Retrying` (the daemon owns retry,
//! ARCHITECTURE section 7). A [`LocalTurns`] runs the turns of one session that is pinned to a
//! local model: chat (validated and repaired when the request's shape asks), embeddings and
//! computer-use steps. Events go down a channel of [`TurnStep`]s, the reply is the return value,
//! and the engine's owner is told the engine is in use ([`EngineUse`]) while a turn runs.
//!
//! What it leaves to its caller: which model the session is pinned to, a hosted model, and a
//! speech turn (a speech engine is not an OpenAI-compatible one).

use crate::bridge::{self, Frames};
use crate::cua_run::{CuaRun, StepJob};
use crate::structured::{self, Shaping};
use engine_supervisor::EngineId;
use model_http::{HttpClient, Timeouts, WaitMs as HttpWaitMs};
use model_openai_compat::{Flavor, OpenAiCodec, OpenAiCompat};
use model_provider as sp;
use model_provider::{Embedder, Retrying};
use porter_infer::{
    ChatSink, CuaBegin, CuaStepFailure, CuaStepReply, EmbedReply, Flow, InferEvent, InferRefusal,
    InferReply, ModelError, ServedBy, TokenUsage,
};
use porter_router::local::LocalModel;
use porter_router::seams::TurnStep;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Waits with the clock of the runtime, so a test with paused time does not wait.
#[derive(Debug, Clone, Copy)]
pub struct TokioSleep;

impl sp::Sleeper for TokioSleep {
    fn sleep(&self, wait: sp::WaitMs) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(Duration::from_millis(u64::from(wait.0)))
    }
}

/// Three attempts, a quarter of a second doubling to four seconds (porter-bridge's, shared with
/// porter-client's in-process engines).
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
pub async fn run_chat<P: sp::Provider>(
    provider: &P,
    turn: &sp::TurnRequest,
    served: &ServedBy,
    events: impl FnMut(InferEvent) -> sp::Flow + Send,
    touch: impl FnMut() + Send,
) -> (InferReply, Option<bridge::NoScores>) {
    let mut sink = Forward {
        events,
        touch,
        last_touch: Instant::now(),
        gathered: bridge::Gathered::for_turn(turn),
    };
    match provider.turn(turn, &mut sink).await {
        Ok(end) => {
            let (reply, why) = sink.gathered.chat_reply_noted(&end, served.clone());
            (InferReply::Chat(reply), why)
        }
        Err(error) => (InferReply::Failed(bridge::model_error(&error)), None),
    }
}

/// Says in the daemon's log why a reply that asked for option shares has none: the reply is the
/// same without them and the app is not told (a Choice's answer never depends on its scores).
pub fn note_no_scores(why: Option<bridge::NoScores>) {
    if let Some(why) = why {
        eprintln!("inferd: choice scores: none: {why}");
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
        InferReply::CuaStep(CuaStepReply {
            thought: None,
            actions: Vec::new(),
            dropped: Vec::new(),
            safety: Vec::new(),
        })
    }

    /// One chat turn on `model`, `shaping` saying whether its reply is validated and repaired.
    pub async fn chat(
        &self,
        model: &LocalModel,
        served: &ServedBy,
        turn: &sp::TurnRequest,
        shaping: Shaping,
    ) -> InferReply {
        let Some(provider) = chat_provider(model) else {
            return refused(InferRefusal::Unsupported);
        };
        if let Shaping::Checked(checked) = shaping {
            return self.checked(&provider, model, served, *checked, turn).await;
        }
        let steps = &self.steps;
        let (reply, why) = run_chat(
            &provider,
            turn,
            served,
            |event| send(steps, event),
            || self.engines.used(&model.spec.id),
        )
        .await;
        note_no_scores(why);
        reply
    }

    /// A turn whose reply is validated, and repaired once, before the app is told it.
    async fn checked(
        &self,
        provider: &impl sp::Provider,
        model: &LocalModel,
        served: &ServedBy,
        checked: structured::Checked,
        turn: &sp::TurnRequest,
    ) -> InferReply {
        let steps = &self.steps;
        let mut forward = |event| send(steps, event);
        let touch = || self.engines.used(&model.spec.id);
        match structured::run(provider, checked, turn, &mut forward, touch).await {
            Ok(valid) => {
                let _ = send(steps, InferEvent::TextDelta(valid.text.clone()));
                let scores = match bridge::turn_scores(turn, valid.first_token.as_ref()) {
                    Some(Ok(scores)) => Some(scores),
                    Some(Err(why)) => {
                        note_no_scores(Some(why));
                        None
                    }
                    None => None,
                };
                InferReply::Chat(porter_infer::ChatReply {
                    text: valid.text,
                    tool_calls: Vec::new(),
                    stop: porter_infer::StopReason::EndTurn,
                    thought: valid.thought,
                    usage: valid.usage,
                    served: served.clone(),
                    scores,
                })
            }
            Err(error) => InferReply::Failed(error),
        }
    }

    /// An embedding request on `model`, cut into the batches its entry allows.
    pub async fn embed(
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

    /// One computer-use step of the session's run on `model`; refused `Unsupported` when no run
    /// has begun or the model has no chat wire.
    pub async fn cua(
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
        };
        match run.step(job, &mut sink).await {
            Ok(reply) => {
                *self.run.lock().unwrap_or_else(PoisonError::into_inner) = Some(run);
                InferReply::CuaStep(reply)
            }
            Err(CuaStepFailure::Unparseable) => InferReply::Failed(ModelError::Unparseable),
            Err(CuaStepFailure::ModelFailed(error)) => InferReply::Failed(error),
        }
    }
}
