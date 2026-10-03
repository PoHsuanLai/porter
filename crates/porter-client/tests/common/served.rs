//! The real session server (`inferd::serve`) over scripted seams, so the client's tests also
//! run against the real session machine: `Routed` and `Waiting` come from it, a queued request
//! queues, and a computer-use step before a begin is refused.

use inferd::serve::{
    AuditSink, EngineFailed, EngineHost, Router, RunningTurn, TurnRunner, TurnStep,
};
use inferd::session::{RouteDecision, SessionSpec};
use porter_fake::{Script, ScriptStep};
use porter_infer::{
    AudioFrame, InferEvent, InferRefusal, InferReply, InferRequest, ModelRef, ServedBy,
};
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// The route is fixed.
#[derive(Debug, Clone)]
pub struct Route(pub Result<RouteDecision, InferRefusal>);

impl Router for Route {
    async fn route(&self, _spec: &SessionSpec) -> Result<RouteDecision, InferRefusal> {
        self.0.clone()
    }
}

/// The engine is ready when `gate` says so (at once without one).
#[derive(Debug, Clone, Default)]
pub struct Gate(pub Option<Arc<Notify>>);

impl EngineHost for Gate {
    async fn want(&self, _model: ModelRef) -> Result<(), EngineFailed> {
        if let Some(gate) = &self.0 {
            gate.notified().await;
        }
        Ok(())
    }

    fn release(&self, _model: &ModelRef) {}
}

/// Turns that play a `porter_fake::Script` of their request kind: `Emit` steps at once,
/// `AfterAudio` steps when the audio ends. A request with no script finishes `Unsupported`.
#[derive(Debug, Clone, Default)]
pub struct Scripted {
    scripts: Arc<Mutex<VecDeque<Script>>>,
    /// Every request the engine was given, with how many descriptors came with it.
    pub seen: Arc<Mutex<Vec<(InferRequest, usize)>>>,
}

impl Scripted {
    pub fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into())),
            seen: Arc::default(),
        }
    }
}

#[derive(Debug)]
pub struct Played {
    now: VecDeque<InferEvent>,
    after_audio: VecDeque<InferEvent>,
}

impl TurnRunner for Scripted {
    type Turn = Played;

    fn start(&self, request: InferRequest, attachments: Vec<OwnedFd>) -> Played {
        self.seen
            .lock()
            .expect("lock")
            .push((request.clone(), attachments.len()));
        let mut scripts = self.scripts.lock().expect("lock");
        let at = scripts.iter().position(|s| s.kind == request.kind());
        let steps = at
            .and_then(|i| scripts.remove(i))
            .map(|s| s.steps)
            .unwrap_or_else(|| {
                vec![ScriptStep::Emit(InferEvent::Finished(InferReply::Refused(
                    InferRefusal::Unsupported,
                )))]
            });
        let (now, later): (Vec<_>, Vec<_>) = steps
            .into_iter()
            .partition(|step| matches!(step, ScriptStep::Emit(_)));
        let event = |step| match step {
            ScriptStep::Emit(event) | ScriptStep::AfterAudio { event, .. } => event,
        };
        Played {
            now: now.into_iter().map(event).collect(),
            after_audio: later.into_iter().map(event).collect(),
        }
    }
}

impl RunningTurn for Played {
    fn audio(&mut self, _frame: AudioFrame) {}

    fn end_audio(&mut self) {
        self.now.extend(self.after_audio.drain(..));
    }

    async fn next(&mut self) -> TurnStep {
        match self.now.pop_front() {
            Some(InferEvent::Finished(reply)) => TurnStep::Done(reply),
            Some(event) => TurnStep::Event(event),
            None => std::future::pending().await,
        }
    }
}

/// Audit entries are not the client's business here.
#[derive(Debug, Clone, Default)]
pub struct Unaudited;

impl AuditSink for Unaudited {
    fn record(&self, _spec: &SessionSpec, _served: &ServedBy, _reply: &InferReply) {}
}
