//! Computer-use steps inside inferd: `cua-session` (stoker) assembles the prompt from the goal,
//! the history and a frame, runs one model turn through the pinned engine, parses and maps the
//! reply into window space. The history lives in the session, so the session is pinned to one
//! model.

use cua_action::WindowSpace;
use porter_core::DataClass;
use porter_infer::{
    ChatSink, CuaBegin, CuaStepFailure, CuaStepReply, CuaStepRequest, InferRefusal,
};

/// One run's state on one session: its goal and history.
#[derive(Debug)]
pub struct CuaRun {
    begin: CuaBegin,
}

/// Only `Screen` data may enter a computer-use session: the frames are the screen.
pub fn check_class(class: DataClass) -> Result<(), InferRefusal> {
    match class {
        DataClass::Screen => Ok(()),
        _ => Err(InferRefusal::Unsupported),
    }
}

impl CuaRun {
    /// A run for this goal.
    pub fn begin(begin: CuaBegin) -> Self {
        Self { begin }
    }

    /// The window-space actions for one step: prepare the frame, one model turn (streaming
    /// `ActionProposed` into `sink`), parse, map; one repair prompt is allowed.
    ///
    /// A stub: this signature reaches neither the pinned model's engine nor the bytes behind
    /// `ImageSource::Attached` (FINDINGS "Fill F3: inferd", interface ask 1). `cua_step::step`
    /// runs a step meanwhile, for the tool dialects.
    pub async fn step(
        &mut self,
        request: &CuaStepRequest,
        sink: &mut impl ChatSink,
    ) -> Result<CuaStepReply, CuaStepFailure> {
        let _ = (
            &self.begin,
            request,
            sink,
            std::marker::PhantomData::<WindowSpace>,
        );
        todo!("cua_session::CuaSession::request, the model turn, absorb, one repair")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_screen_data_enters_a_computer_use_session() {
        assert_eq!(check_class(DataClass::Screen), Ok(()));
        for class in [
            DataClass::Mail,
            DataClass::Voice,
            DataClass::Public,
            DataClass::AppOwn,
        ] {
            assert_eq!(
                check_class(class),
                Err(InferRefusal::Unsupported),
                "{class:?}"
            );
        }
    }
}
