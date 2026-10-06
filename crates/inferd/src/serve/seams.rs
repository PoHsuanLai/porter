//! What the session server leans on, as traits the daemon fills: who answers (the route), the
//! engines behind it, the model turn itself and the audit trail. Async seams return
//! `impl Future + Send`, as everywhere in porter; none needs `dyn`.

use super::carried::Carried;
use crate::session::{HeardAudio, RouteDecision, Routing, SessionSpec};
use porter_infer::{
    AudioFrame, ChatRequest, InferEvent, InferRefusal, InferReply, InferRequest, ModelRef,
    PickRefusal, Why,
};
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

    /// `route` with the reason, and the named model that could not serve when that is why it
    /// refused. A router that has no reason to give is as good as named and announces nothing.
    fn route_why(
        &self,
        spec: &SessionSpec,
    ) -> impl Future<Output = Result<Routing, PickRefusal>> + Send {
        async move {
            self.route(spec)
                .await
                .map(Routing::from)
                .map_err(PickRefusal::from)
        }
    }
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

    /// Starts the turn of a voice chat: `chat` is answered over what `heard` says, through a
    /// pipeline (the `Hear` stage reads the audio, then the answering model gets the transcript).
    /// The turn announces its own stages; its events and reply are those of any chat turn.
    fn start_heard(&self, chat: ChatRequest, heard: HeardAudio) -> Self::Turn;
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

    /// `record` with the route's reason, which the audit entry keeps beside the model. A sink
    /// that does not keep it drops it.
    fn record_why(
        &self,
        spec: &SessionSpec,
        served: &porter_infer::ServedBy,
        reply: &InferReply,
        carried: &Carried,
        why: &Why,
    ) {
        let _ = why;
        self.record(spec, served, reply, carried);
    }
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
