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
//! Interim: stoker's `ObservationIn` has no place for the window's contents or for notes said to
//! the run (interface ask 122), so they go in a text part of the step's user message, in the words
//! inferd used before `cua-session` was filled.

use crate::bridge::{self, BridgeError, Frames};
use crate::local::LocalModel;
use crate::tee::{Echo, Tee};
use cua_action::CuaDialect;
use cua_session::{
    CuaProfile, CuaSession, CuaTaskText, FrameBudget, MaskedRegions, ObservationIn, RepairBudget,
    StepIndex, StepLines, StepOutcome, TurnSettings,
};
use cua_vendors::StepResult;
use model_provider::{
    Batching, CuaSupport, Flow, ImageInput, Limits, Part, Provider, Reasoning, Role, ToolCallId,
    ToolParallelism, TurnRequest,
};
use porter_infer::{
    CuaBegin, CuaStepFailure, CuaStepReply, CuaStepRequest, DropReason, DroppedAction, FrameLayout,
    InferEvent, MediaKind, ModelError, NoteFrom, PrevResult, SafetyHint, StepNote, TreeText,
};
use vision_prep::{Encoding, FrameMap, MediaType, RawFrame, prepare};

/// A step that gave no actions: why, and the session when the step still counts (a reply that
/// never parsed is a step the model took, so the next prompt lists it).
#[derive(Debug)]
pub struct Failed {
    /// What the app is told.
    pub failure: CuaStepFailure,
    /// The session after the step, for a step that counts; none for one that does not (the same
    /// step can be asked again).
    pub kept: Option<Box<CuaSession>>,
}

impl From<CuaStepFailure> for Failed {
    fn from(failure: CuaStepFailure) -> Self {
        Self {
            failure,
            kept: None,
        }
    }
}

fn unreadable() -> Failed {
    CuaStepFailure::ModelFailed(ModelError::Unreadable).into()
}

/// The images a prompt may hold are `history + 1` (the past frames and this one).
fn history_of(per_prompt: u16) -> FrameBudget {
    FrameBudget(u8::try_from(per_prompt.saturating_sub(1)).unwrap_or(u8::MAX))
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
    let profile = CuaProfile {
        dialect,
        rule: caps.images.rule,
        space: caps.images.space,
        history: history_of(caps.images.per_prompt.0),
        repair: RepairBudget(1),
        encoding: Encoding::Png,
    };
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

fn note_text(note: &StepNote) -> String {
    let who = match note.from {
        NoteFrom::Person => "The person says",
        NoteFrom::Agent => "A helper says",
    };
    format!("{who}: {}", note.text)
}

/// The window's contents and the notes said to the run, as lines; none when there are neither.
fn context_text(request: &CuaStepRequest) -> Option<String> {
    let mut lines: Vec<String> = request.notes.iter().map(note_text).collect();
    if let TreeText::Present(tree) = &request.tree {
        lines.push(format!("Window contents:\n{tree}"));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Puts `text` in the step's user message, after the lines stoker wrote and before the frames.
fn add_context(turn: &mut TurnRequest, text: String) {
    let Some(user) = turn
        .messages
        .iter_mut()
        .rev()
        .find(|m| m.role == Role::User)
    else {
        return;
    };
    let at = user
        .parts
        .iter()
        .position(|part| matches!(part, Part::Text(_)))
        .map_or(0, |first| first + 1);
    user.parts.insert(at, Part::Text(text));
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

/// One step: the reply and the session with the step remembered. Thoughts stream into `forward`
/// as the turn runs, then an `ActionProposed` for each action; a `Stop` from it ends the turn
/// (nobody is listening) or the proposals. `session` is the run's, which the caller keeps its own
/// copy of: a step that fails leaves the run as it was.
pub async fn step<P: Provider>(
    session: CuaSession,
    model: &LocalModel,
    provider: &P,
    request: &CuaStepRequest,
    frames: &Frames,
    mut forward: impl FnMut(InferEvent) -> Flow + Send,
) -> Result<(CuaStepReply, CuaSession), Failed> {
    let caps = model.caps().ok_or_else(unreadable)?;
    let map = FrameMap::new(
        request.window.logical,
        request.window.scale,
        &caps.images.rule,
        caps.images.space,
    )
    .map_err(|_| unreadable())?;
    let image = frame_image(request, frames, &map).map_err(|_| unreadable())?;
    let observation = ObservationIn {
        step: StepIndex(request.step.0),
        cursor: request.cursor,
        prev: prev_results(&request.prev),
        masked: MaskedRegions(request.masked.0),
    };
    let mut sent = session.request(&observation, &map, image);
    if let Some(text) = context_text(request) {
        add_context(&mut sent, text);
    }
    let mut session = session;
    loop {
        let mut tee = Tee::new(Echo::ThoughtsAndText, &mut forward);
        let end = provider
            .turn(&sent, &mut tee)
            .await
            .map_err(|error| CuaStepFailure::ModelFailed(bridge::model_error(&error)))?;
        if tee.flow() == Flow::Stop {
            // The session is gone: the cut turn holds no call, and a repair would be asked of nobody.
            return Err(CuaStepFailure::Unparseable.into());
        }
        let (next, outcome) = session.absorb_for(&sent, tee.finish(end), &map);
        session = next;
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
                return Ok((reply, session));
            }
            StepOutcome::Repair(next) => sent = next,
            StepOutcome::Unparseable(_) => {
                return Err(Failed {
                    failure: CuaStepFailure::Unparseable,
                    kept: Some(Box::new(session)),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
