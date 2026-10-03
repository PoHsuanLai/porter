//! What the session server leans on, as traits the daemon fills: who answers (the route), the
//! engines behind it, the model turn itself and the audit trail. Async seams return
//! `impl Future + Send`, as everywhere in porter; none needs `dyn`.

use super::carried::Carried;
use crate::session::{RouteDecision, SessionSpec};
use porter_infer::{AudioFrame, InferEvent, InferRefusal, InferReply, InferRequest, ModelRef};
use std::future::Future;
use std::os::fd::OwnedFd;

/// Decides who answers a session: policy, grants, spend and readiness. One call per session;
/// the route is not decided again, so the session stays pinned to one model.
pub trait Router: Send + Sync {
    /// The model to use, or why none may.
    fn route(
        &self,
        spec: &SessionSpec,
    ) -> impl Future<Output = Result<RouteDecision, InferRefusal>> + Send;
}

/// The engine could not be brought up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineFailed;

/// Brings an engine up for a session and gives it back.
pub trait EngineHost: Send + Sync {
    /// Resolves when the model's engine is ready to answer (starting it if it must), or fails.
    /// Dropping the future abandons the wait.
    fn want(&self, model: ModelRef) -> impl Future<Output = Result<(), EngineFailed>> + Send;

    /// The session no longer needs the engine.
    fn release(&self, model: &ModelRef);
}

/// What a running turn produces next.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnStep {
    /// A delta, a tool call, usage, heard text or speech.
    Event(InferEvent),
    /// The turn ended with this reply; it is the turn's last step.
    Done(InferReply),
}

/// One model turn in flight. Dropping it ends the turn and closes the engine's stream.
pub trait RunningTurn: Send {
    /// A piece of the person's voice for a `Transcribe` turn (checked by the session machine).
    fn audio(&mut self, frame: AudioFrame);

    /// The person stopped talking.
    fn end_audio(&mut self);

    /// The next step; never called again after `Done`.
    fn next(&mut self) -> impl Future<Output = TurnStep> + Send;
}

/// Starts model turns.
pub trait TurnRunner: Send + Sync {
    /// The turn it starts.
    type Turn: RunningTurn;

    /// Starts `request`. `attachments` are the descriptors (memfds) that arrived with its
    /// frame: the request names them by index (`ImageSource::Attached`), and exactly as many as
    /// it names are given.
    fn start(&self, request: InferRequest, attachments: Vec<OwnedFd>) -> Self::Turn;
}

/// Where the finished turns are recorded (spend and the audit entry; never content).
pub trait AuditSink: Send + Sync {
    /// One turn ended with `reply` on a session of `spec` pinned to `served`; `carried` says what
    /// its request held (how many frames, how much audio), which the reply does not.
    fn record(
        &self,
        spec: &SessionSpec,
        served: &porter_infer::ServedBy,
        reply: &InferReply,
        carried: &Carried,
    );
}

/// The four seams one session server uses.
#[derive(Debug)]
pub struct Seams<R, E, T, A> {
    /// The route.
    pub router: R,
    /// The engines.
    pub engines: E,
    /// The model turns.
    pub runner: T,
    /// The audit trail.
    pub audit: A,
}
