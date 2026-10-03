//! Computer-use steps inside inferd, as one run on one session: its goal and what it did so
//! far. A step runs one model turn through the pinned engine; the prompt, the parse (with one
//! repair) and the mapping into window space are stoker's `CuaSession`, which `cua_step` drives.
//! The history lives in the session value the run holds, so the run is pinned to one model.

use crate::bridge::Frames;
use crate::cua_step::{self, Failed};
use crate::local::LocalModel;
use cua_session::CuaSession;
use model_provider::{Flow as ProviderFlow, Provider};
use porter_core::DataClass;
use porter_infer::{
    ChatSink, CuaBegin, CuaStepFailure, CuaStepReply, CuaStepRequest, Flow, InferRefusal,
    ModelError,
};

/// One run's state on one session: its goal, and the stoker session once the first step has
/// chosen the model's profile (the model is the session's pin, known to the step and not to the
/// `CuaBegin` request).
#[derive(Clone)]
pub struct CuaRun {
    begin: CuaBegin,
    session: Option<CuaSession>,
}

// The goal is what the person asked for: Debug shows sizes only.
impl std::fmt::Debug for CuaRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CuaRun({:?}, {} remembered steps)",
            self.begin,
            self.session.as_ref().map_or(0, CuaSession::remembered)
        )
    }
}

/// Everything one step needs beyond the run: the pinned model, the provider that reaches its
/// engine, the bytes behind the request's `ImageSource::Attached` and the request itself.
#[derive(Debug)]
pub struct StepJob<'a, P> {
    /// The model the session is pinned to.
    pub model: &'a LocalModel,
    /// The engine's provider.
    pub provider: &'a P,
    /// The descriptors that arrived with the request's frame.
    pub frames: &'a Frames,
    /// The step.
    pub request: &'a CuaStepRequest,
}

/// Only `Screen` data may enter a computer-use session: the frames are the screen.
pub fn check_class(class: DataClass) -> Result<(), InferRefusal> {
    match class {
        DataClass::Screen => Ok(()),
        _ => Err(InferRefusal::Unsupported),
    }
}

/// The provider's flow control for the session's.
fn provider_flow(flow: Flow) -> ProviderFlow {
    match flow {
        Flow::Continue => ProviderFlow::Continue,
        Flow::Stop => ProviderFlow::Stop,
    }
}

impl CuaRun {
    /// A run for this goal, with nothing done.
    pub fn begin(begin: CuaBegin) -> Self {
        Self {
            begin,
            session: None,
        }
    }

    /// The window-space actions for one step: prepare the frame, one model turn (thoughts as
    /// they stream, then `ActionProposed` for each action, into `sink`; a `Stop` from it ends
    /// the proposals), parse (one more turn when the reply must be repaired), map. A step that
    /// succeeds is remembered in the run, and so is one whose reply never parsed (the model took
    /// it); one that fails otherwise is not, so the same step can be asked again.
    pub async fn step<P: Provider>(
        &mut self,
        job: StepJob<'_, P>,
        sink: &mut impl ChatSink,
    ) -> Result<CuaStepReply, CuaStepFailure> {
        let session = match &self.session {
            Some(session) => session.clone(),
            None => cua_step::open(job.model, &self.begin)
                .ok_or(CuaStepFailure::ModelFailed(ModelError::Unreadable))?,
        };
        let forward = |event| provider_flow(sink.event(event));
        match cua_step::step(
            session,
            job.model,
            job.provider,
            job.request,
            job.frames,
            forward,
        )
        .await
        {
            Ok((reply, next)) => {
                self.session = Some(next);
                Ok(reply)
            }
            Err(Failed { failure, kept }) => {
                self.session = kept.map(|kept| *kept).or_else(|| self.session.take());
                Err(failure)
            }
        }
    }
}

#[cfg(test)]
mod tests;
