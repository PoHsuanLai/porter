//! Computer-use steps inside inferd, as one run on one session: its goal and what it did so
//! far. A step runs one model turn through the pinned engine, parses and maps the reply into
//! window space (`cua_step` holds the turn today for the tool dialects, and `cua-session` of
//! stoker takes the prompt and the parse over when its bodies exist). The history lives in the
//! run, so the session is pinned to one model.

use crate::bridge::Frames;
use crate::cua_step::{self, Run};
use crate::local::LocalModel;
use model_provider::{Flow as ProviderFlow, Provider};
use porter_core::DataClass;
use porter_infer::{
    ChatSink, CuaBegin, CuaStepFailure, CuaStepReply, CuaStepRequest, Flow, InferRefusal,
};

/// One run's state on one session: its goal and history.
#[derive(Debug, Clone)]
pub struct CuaRun {
    run: Run,
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
            run: Run::new(begin),
        }
    }

    /// The window-space actions for one step: prepare the frame, one model turn (thoughts as
    /// they stream, then `ActionProposed` for each action, into `sink`; a `Stop` from it ends
    /// the proposals), parse, map. A step that succeeds is remembered in the run; one that fails
    /// is not, so the same step can be asked again.
    pub async fn step<P: Provider>(
        &mut self,
        job: StepJob<'_, P>,
        sink: &mut impl ChatSink,
    ) -> Result<CuaStepReply, CuaStepFailure> {
        let forward = |event| provider_flow(sink.event(event));
        let (reply, next) = cua_step::step(
            &self.run,
            job.model,
            job.provider,
            job.request,
            job.frames,
            forward,
        )
        .await?;
        self.run = next;
        Ok(reply)
    }
}

#[cfg(test)]
mod tests;
