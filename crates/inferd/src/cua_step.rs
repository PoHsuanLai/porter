//! One computer-use step over a model turn: stoker's `CuaSession` builds the prompt and reads the
//! reply, and this module is the turn between them. It prepares the frame, runs the turn through
//! a transcript sink, hands the transcript to `absorb_for` and, when that asks for a repair, runs
//! one more turn with the request it returns.
//!
//! What stoker leaves to the daemon is done here, from the model's catalog entry: the
//! `CuaProfile` (dialect, resize rule and space of the entry, a history of one frame fewer than
//! the model takes per prompt, one repair) and the `TurnSettings` (the entry's reasoning-off
//! sampling, its output limit, no reasoning, how many calls a turn may make, the engine's extras).
//!
//! The window's contents and the notes said to the run are `ObservationIn::with_tree` and
//! `with_notes`; a note keeps who said it in its words ("The person says: ..."), because stoker's
//! `StepNote` is one line of text.

use crate::bridge::{self, BridgeError, Frames};
use crate::local::LocalModel;
use crate::tee::{Echo, Tee};
use cua_action::CuaDialect;
use cua_session::{
    CuaProfile, CuaSession, CuaTaskText, FrameBudget, MaskedRegions, ObservationIn, RepairBudget,
    StepIndex, StepLines, StepOutcome, TurnSettings,
};
use cua_session::{StepNote as NoteLine, TreeText as WindowText};
use cua_vendors::StepResult;
use model_provider::{
    Batching, CuaSupport, Flow, ImageInput, Limits, Provider, Reasoning, ToolCallId,
    ToolParallelism,
};
use porter_infer::{
    CuaBegin, CuaStepFailure, CuaStepReply, CuaStepRequest, DropReason, DroppedAction, FrameLayout,
    InferEvent, MediaKind, ModelError, NoteFrom, PrevResult, SafetyHint, StepNote, TreeText,
};
use vision_prep::{Encoding, FrameMap, MediaType, RawFrame, prepare};

/// A step that gave no actions: why the app is told. A reply that never parsed is a step the model
/// took, which the session (changed in place) already lists; any other failure leaves it as it was.
#[derive(Debug)]
pub struct Failed {
    /// What the app is told.
    pub failure: CuaStepFailure,
}

impl From<CuaStepFailure> for Failed {
    fn from(failure: CuaStepFailure) -> Self {
        Self { failure }
    }
}

fn unreadable() -> Failed {
    CuaStepFailure::ModelFailed(ModelError::Unreadable).into()
}

/// The session for a run on this model, or none when the model has no tool or text dialect (a
/// vendor's wire needs the vendor's backend, not an engine on this computer).
pub fn open(model: &LocalModel, begin: &CuaBegin) -> Option<CuaSession> {
    let caps = model.caps()?;
    let CuaSupport::Dialect {
        dialect, batching, ..
    } = caps.computer_use
    else {
        return None;
    };
    if matches!(dialect, CuaDialect::Wire(_)) {
        return None;
    }
    // As many past frames as the model takes with the current one (`per_prompt - 1`); stoker cuts
    // the wish to what the entry allows.
    let profile = CuaProfile::for_model(
        dialect,
        &caps.images,
        FrameBudget::within(caps.images.per_prompt),
        RepairBudget(1),
        Encoding::Png,
    );
    let settings = TurnSettings {
        sampling: model.entry.sampling?.reasoning_off,
        limits: Limits {
            max_output: caps.max_output,
            stop: Vec::new(),
        },
        reasoning: Reasoning::Off,
        tool_calls: match batching {
            Batching::One => ToolParallelism::One,
            Batching::Many => ToolParallelism::Many,
        },
        engine: bridge::extras(model.flavor),
        lines: StepLines(8),
    };
    let task = CuaTaskText {
        goal: begin.goal.clone(),
        hints: begin.hints.clone(),
    };
    Some(CuaSession::begin(profile, task, model.name.clone()).with_settings(settings))
}

/// What happened to each action of the previous step, as stoker's prompt has it. The calls'
/// ids are ours (a tool or text dialect does not read them; a vendor wire is not served here).
/// A failure, and the person's own part in it, are told as a refusal with their words.
fn prev_results(prev: &[PrevResult]) -> Vec<StepResult> {
    prev.iter()
        .enumerate()
        .map(|(index, result)| {
            let id = ToolCallId(format!("prev-{index}"));
            match result {
                PrevResult::Done => StepResult::Done(id),
                PrevResult::NotRun => StepResult::NotRun(id),
                PrevResult::Refused(why) | PrevResult::Failed(why) => StepResult::Refused {
                    id,
                    why: why.clone(),
                },
                PrevResult::UserDeclined => StepResult::Refused {
                    id,
                    why: "the person declined it".to_owned(),
                },
                PrevResult::UserActed => StepResult::Refused {
                    id,
                    why: "the person did it themselves".to_owned(),
                },
            }
        })
        .collect()
}

fn note_line(note: &StepNote) -> NoteLine {
    let who = match note.from {
        NoteFrom::Person => "The person says",
        NoteFrom::Agent => "A helper says",
    };
    NoteLine(format!("{who}: {}", note.text))
}

/// What the runner saw before this step, with the window's contents and the notes when there are
/// any.
fn observation(request: &CuaStepRequest) -> ObservationIn {
    let seen = ObservationIn::new(
        StepIndex(request.step.0),
        request.cursor,
        prev_results(&request.prev),
        MaskedRegions(request.masked.0),
    )
    .with_notes(request.notes.iter().map(note_line).collect());
    match &request.tree {
        TreeText::Present(tree) => seen.with_tree(WindowText(tree.clone())),
        TreeText::Absent => seen,
    }
}

/// The frame as an image the model is shown.
fn frame_image(
    request: &CuaStepRequest,
    frames: &Frames,
    map: &FrameMap,
) -> Result<ImageInput, BridgeError> {
    let bytes = frames.resolve(&request.frame.source)?;
    match request.frame.layout {
        FrameLayout::Encoded(kind) => {
            let media = match kind {
                MediaKind::Png => MediaType::Png,
                MediaKind::Jpeg => MediaType::Jpeg,
            };
            Ok(bridge::image_input(media, bytes))
        }
        FrameLayout::Raw {
            format,
            size,
            stride,
        } => {
            let raw = RawFrame {
                pixels: &bytes,
                size,
                stride,
                format,
            };
            let prepared = prepare(raw, map, Encoding::Png).map_err(|_| BridgeError::Attachment)?;
            Ok(bridge::image_input(prepared.media, prepared.bytes))
        }
    }
}

fn dropped_reason(reason: cua_parse::DropReason) -> DropReason {
    match reason {
        cua_parse::DropReason::UnsupportedVerb => DropReason::UnsupportedVerb,
        cua_parse::DropReason::MissingArgument => DropReason::MissingArgument,
        cua_parse::DropReason::BadArgument => DropReason::BadArgument,
        cua_parse::DropReason::BadNumber => DropReason::BadNumber,
        cua_parse::DropReason::TooLong => DropReason::TooLong,
        cua_parse::DropReason::OverBatchLimit => DropReason::OverBatchLimit,
        cua_parse::DropReason::OutOfFrame => DropReason::OutOfFrame,
    }
}

/// One step on the run's own session, which the step changes in place: the reply, with the step
/// remembered. Thoughts stream into `forward` as the turn runs, then an `ActionProposed` for each
/// action; a `Stop` from it ends the turn (nobody is listening) or the proposals. A step that
/// fails before its reply is absorbed leaves the session's history as it was, and one that fails
/// after a repair gets its repair budget back, so the same step can be asked again.
pub async fn step<P: Provider>(
    session: &mut CuaSession,
    model: &LocalModel,
    provider: &P,
    request: &CuaStepRequest,
    frames: &Frames,
    mut forward: impl FnMut(InferEvent) -> Flow + Send,
) -> Result<CuaStepReply, Failed> {
    let caps = model.caps().ok_or_else(unreadable)?;
    let map = FrameMap::new(
        request.window.logical,
        request.window.scale,
        &caps.images.rule,
        caps.images.space,
    )
    .map_err(|_| unreadable())?;
    let image = frame_image(request, frames, &map).map_err(|_| unreadable())?;
    let mut sent = session.request(&observation(request), &map, image);
    loop {
        let mut tee = Tee::new(Echo::ThoughtsAndText, &mut forward);
        let end = match provider.turn(&sent, &mut tee).await {
            Ok(end) => end,
            Err(error) => {
                session.refill_repairs();
                return Err(CuaStepFailure::ModelFailed(bridge::model_error(&error)).into());
            }
        };
        if tee.flow() == Flow::Stop {
            session.refill_repairs();
            // The session is gone: the cut turn holds no call, and a repair would be asked of nobody.
            return Err(CuaStepFailure::Unparseable.into());
        }
        let outcome = session.absorb_for(&sent, tee.finish(end), &map);
        match outcome {
            StepOutcome::Actions {
                thought,
                actions,
                dropped,
            } => {
                for action in &actions {
                    if forward(InferEvent::ActionProposed(action.clone())) == Flow::Stop {
                        break;
                    }
                }
                let reply = CuaStepReply {
                    thought,
                    actions,
                    dropped: dropped
                        .iter()
                        .map(|d| DroppedAction {
                            verb: d.verb.as_str().to_owned(),
                            reason: dropped_reason(d.reason),
                        })
                        .collect(),
                    safety: Vec::<SafetyHint>::new(),
                };
                return Ok(reply);
            }
            StepOutcome::Repair(next) => sent = next,
            StepOutcome::Unparseable(_) => {
                return Err(Failed {
                    failure: CuaStepFailure::Unparseable,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
