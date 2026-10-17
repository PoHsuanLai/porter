//! The model turns of one session. A turn is a task that pushes `TurnStep`s down a channel;
//! dropping the turn aborts the task, which drops the HTTP future and closes the engine's stream.
//! The turns on a model of this computer (stoker's `OpenAiCompat` against the pinned model's
//! engine, wrapped in `Retrying`: inferd owns retry, ARCHITECTURE section 7) are
//! `porter_turns::local::LocalTurns`; what stays here is the daemon's half: what a session is
//! pinned to, a hosted model's turn, and a speech turn.
//!
//! What a session is pinned to is decided by its router and read here through a [`Pin`] cell:
//! `serve_session` calls the router once and the runner never sees the decision otherwise.
//! A `Transcribe` turn runs on the speech host ([`crate::speech::SpeechRunner`]): the session's
//! audio frames are queued to the turn, `end_audio` closes the queue. `Speak` is refused
//! `Unsupported` (no text-to-speech runner yet).

use crate::bridge::{self, Frames};
use crate::cloud::Cloud;
use crate::cloud::turn::{self as hosted, CloudPin};
use crate::local::LocalModel;
use crate::pipeline::Hearing;
use crate::seats::{Gate, Pass, Standing, seats_in};
use crate::serve::{TurnRunner, TurnStep};
use crate::session::HeardAudio;
use crate::speech::{ChannelAudio, SpeechRunner};
use crate::structured::{self, Limits, Shaping};
use crate::supervise::Supervised;
use model_catalog::EngineKind;
use porter_core::Tier;
use porter_infer::{
    Asker, AudioFrame, ChatRequest, InferEvent, InferRefusal, InferReply, InferRequest, ModelError,
    QueueFact, ServedBy, TranscribeBegin,
};
use porter_turns::cua_run::CuaRun;
use porter_turns::local::{LocalTurns, ToSession};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

/// Retry, the engine wait and the turn in flight moved to `porter_turns`; these paths stay for
/// the code that names them here.
pub use porter_turns::Turn;
pub use porter_turns::local::{RETRY, TokioSleep, note_no_scores};

/// What the router decided for one session.
#[derive(Debug, Clone)]
pub struct Pinned {
    /// The account, model and locality (what the client is told and the audit records).
    pub served: ServedBy,
    /// The local model behind it; none for a model that is not served from this computer.
    pub model: Option<Arc<LocalModel>>,
    /// The hosted model behind it, when that is what the session was routed to.
    pub cloud: Option<CloudPin>,
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

    pub(crate) fn get(&self) -> Option<&Pinned> {
        self.0.get()
    }
}

/// Starts the turns of one session.
#[derive(Debug, Clone)]
pub struct Turns {
    pin: Pin,
    engines: Supervised,
    tier: Tier,
    run: Arc<Mutex<Option<CuaRun>>>,
    limits: Limits,
    cloud: Option<Cloud>,
    hearing: Option<Hearing>,
    /// The daemon's seats and whose turn these are; none takes every request at once.
    seats: Option<(Gate, Asker)>,
}

impl Turns {
    /// Turns for a session of this tier, over the cell its router writes.
    pub fn new(pin: Pin, engines: Supervised, tier: Tier) -> Self {
        Self {
            pin,
            engines,
            tier,
            run: Arc::default(),
            limits: Limits::default(),
            cloud: None,
            hearing: None,
            seats: None,
        }
    }

    /// The same turns, taking a seat on the engine of a local model before each request, as
    /// `asker`: the person's turn goes ahead of a helper's when the engine is busy.
    pub fn admitted(self, gate: Gate, asker: Asker) -> Self {
        Self {
            seats: Some((gate, asker)),
            ..self
        }
    }

    /// The same turns, able to answer a voice chat (`start_heard`).
    pub fn hearing(self, hearing: Hearing) -> Self {
        Self {
            hearing: Some(hearing),
            ..self
        }
    }

    /// What the session's route pinned these turns to, once it has decided.
    pub(crate) fn pinned_to_now(&self) -> Option<&Pinned> {
        self.pin.get()
    }

    /// These turns pinned to another model (the answering stage of a pipeline): a fresh cell,
    /// the same engines, limits and hosted reach.
    pub(crate) fn pinned_to(&self, pinned: Pinned) -> Self {
        let pin = Pin::new();
        pin.set(pinned);
        Self {
            pin,
            hearing: None,
            ..self.clone()
        }
    }

    /// The same turns, able to run a session that was routed to a hosted model.
    pub fn hosted(self, cloud: Cloud) -> Self {
        Self {
            cloud: Some(cloud),
            ..self
        }
    }

    /// The same turns under the configured structured-output limits.
    pub fn limited(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }
}

impl TurnRunner for Turns {
    type Turn = Turn;

    fn start(&self, request: InferRequest, attachments: Vec<OwnedFd>) -> Turn {
        let (steps, inbox) = mpsc::unbounded_channel();
        let (audio, frames) = mpsc::unbounded_channel();
        let job = Job {
            pinned: self.pin.get().cloned(),
            engines: self.engines.clone(),
            tier: self.tier,
            run: Arc::clone(&self.run),
            limits: self.limits,
            cloud: self.cloud.clone(),
            seats: self.seats.clone(),
            steps,
        };
        let task = tokio::spawn(async move {
            let reply = job.reply(request, attachments, frames).await;
            let _ = job.steps.send(TurnStep::Done(reply));
        });
        Turn::listening(inbox, task, audio)
    }

    fn start_heard(&self, chat: ChatRequest, heard: HeardAudio) -> Turn {
        let (steps, inbox) = mpsc::unbounded_channel();
        let turns = self.clone();
        let task = tokio::spawn(async move {
            let mut sink = ToSession::new(steps.clone());
            let reply = match &turns.hearing {
                Some(hearing) => hearing.run(&turns, chat, heard, &mut sink).await,
                None => refused(InferRefusal::Unsupported),
            };
            let _ = steps.send(TurnStep::Done(reply));
        });
        Turn::running(inbox, task)
    }
}

struct Job {
    pinned: Option<Pinned>,
    engines: Supervised,
    tier: Tier,
    run: Arc<Mutex<Option<CuaRun>>>,
    limits: Limits,
    cloud: Option<Cloud>,
    seats: Option<(Gate, Asker)>,
    steps: mpsc::UnboundedSender<TurnStep>,
}

fn refused(refusal: InferRefusal) -> InferReply {
    InferReply::Refused(refusal)
}

impl Job {
    /// The turns on a local model: this session's engines, events and computer-use run.
    fn local(&self) -> LocalTurns<Supervised> {
        LocalTurns::new(
            self.engines.clone(),
            self.steps.clone(),
            Arc::clone(&self.run),
        )
    }

    /// Takes a seat on the model's engine when it is one this daemon launches to chat, telling
    /// the session where it stands while it waits. The seat is held until the pass is dropped,
    /// when the reply is over, cancelled or failed.
    async fn seat(&self, model: &LocalModel) -> Option<Pass> {
        let (gate, asker) = self.seats.as_ref()?;
        let launched = model.attached.is_none()
            && model.loopback.is_none()
            && model.cassette.is_none()
            && matches!(
                model.profile.kind,
                EngineKind::Vllm | EngineKind::LlamaServer
            );
        if !launched {
            return None;
        }
        let seats = seats_in(model.profile.kind, &model.profile.args);
        let mut pass = gate.pass(&model.spec.id, seats, asker);
        let mut told = false;
        while let Standing::Waiting { ahead } = pass.standing() {
            told = true;
            self.tell(QueueFact::Waiting { ahead });
            if !pass.moved().await {
                break;
            }
        }
        if told {
            self.tell(QueueFact::Started);
        }
        Some(pass)
    }

    fn tell(&self, fact: QueueFact) {
        let _ = self.steps.send(TurnStep::Event(InferEvent::Queue(fact)));
    }

    async fn reply(
        &self,
        request: InferRequest,
        attachments: Vec<OwnedFd>,
        audio: mpsc::UnboundedReceiver<AudioFrame>,
    ) -> InferReply {
        if let (
            Some(Pinned {
                served,
                cloud: Some(pin),
                ..
            }),
            Some(cloud),
        ) = (&self.pinned, &self.cloud)
        {
            return hosted::reply(
                cloud,
                pin,
                served,
                self.tier,
                self.steps.clone(),
                &request,
                attachments,
            )
            .await;
        }
        let Some(Pinned {
            served,
            model: Some(model),
            ..
        }) = &self.pinned
        else {
            return InferReply::Failed(ModelError::Unreachable);
        };
        let _seat = self.seat(model).await;
        self.engines.used(&model.spec.id);
        let frames = match Frames::read(attachments) {
            Ok(frames) => frames,
            Err(_) => return InferReply::Failed(ModelError::Unreadable),
        };
        let local = self.local();
        match request {
            InferRequest::Chat(chat) => match bridge::chat_turn(model, &chat, &frames) {
                Ok(turn) => {
                    let shaping = structured::shaping(model, &chat, self.limits);
                    local.chat(model, served, &turn, shaping).await
                }
                Err(_) => refused(InferRefusal::Unsupported),
            },
            InferRequest::Task(task) => match bridge::task_turn(model, &task, self.tier) {
                Ok(turn) => local.chat(model, served, &turn, Shaping::Unchecked).await,
                Err(_) => refused(InferRefusal::Unsupported),
            },
            InferRequest::Embed(embed) => local.embed(model, served, &embed).await,
            InferRequest::CuaBegin(begin) => local.begin_cua(begin),
            InferRequest::CuaStep(step) => local.cua(model, &step, &frames).await,
            InferRequest::Transcribe(begin) => {
                self.transcribe(model, served, &begin, ChannelAudio(audio))
                    .await
            }
            InferRequest::Speak(_) => refused(InferRefusal::Unsupported),
            // a variant a newer porter adds: refused as unsupported
            _ => refused(InferRefusal::Unsupported),
        }
    }

    /// Hears the session's audio on the model's speech host and tells the app what it heard.
    async fn transcribe(
        &self,
        model: &LocalModel,
        served: &ServedBy,
        begin: &TranscribeBegin,
        mut audio: ChannelAudio,
    ) -> InferReply {
        let Some(runner) = SpeechRunner::for_model(model, served.clone(), self.engines.clone())
        else {
            return refused(InferRefusal::Unsupported);
        };
        let mut sink = ToSession::new(self.steps.clone());
        match runner.transcribe(begin, &mut audio, &mut sink).await {
            Ok(reply) => InferReply::Transcribed(reply),
            Err(error) => InferReply::Failed(error),
        }
    }
}

#[cfg(test)]
mod tests;
