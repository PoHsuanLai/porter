//! The streaming session on the `Open` fd: frames the client writes, events inferd writes.

use crate::pick::{Declined, Why};
use crate::pipeline::StageRole;
use crate::readiness::Readiness;
use crate::reply::{InferReply, ServedBy, TokenUsage};
use crate::request::{InferRequest, ToolCallPart};
use crate::speech::{AudioFrame, AudioFrameOut, HeardDelta};
use cua_action::{CuaAction, WindowSpace};
use serde::{Deserialize, Serialize};

/// A frame from the client to inferd. Audio frames follow a `Transcribe` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ClientFrame {
    /// Starts a turn. One turn at a time; a second request queues (depth one).
    Request(InferRequest),
    /// Ends the running turn; its partial usage is still audited.
    Cancel,
    /// A piece of the person's voice for the running `Transcribe` turn.
    Audio(AudioFrame),
    /// No more audio: the turn may finish.
    EndOfAudio,
}

impl ClientFrame {
    /// How many descriptors ride with this frame (see [`InferRequest::attachments`]).
    pub fn attachments(&self) -> usize {
        match self {
            ClientFrame::Request(request) => request.attachments(),
            ClientFrame::Cancel | ClientFrame::Audio(_) | ClientFrame::EndOfAudio => 0,
        }
    }
}

/// An event from inferd to the client. Exactly one `Finished` ends each turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InferEvent {
    /// Who will answer: the "sent to <provider>" indicator and the orb read it.
    Routed(ServedBy),
    /// Why that model answers (a closed set of facts), sent when the route is decided, before
    /// any `Waiting` and before `Routed`. `Why::Evicted` names the idle model being unloaded: a
    /// swap is never silent. Additive: a reader that does not know it must skip it.
    Why(Why),
    /// The model the person named cannot serve, and why; `Finished(Refused(..))` follows. No
    /// other model answers in its place.
    Declined(Declined),
    /// One stage of a pipeline and who runs it, sent before that stage runs: the footer reads
    /// them ("Heard by Whisper, Answered by Gemma 4"). Every answer sends an `Answer` note, a
    /// plain one-stage answer included: it comes after `Routed` (and `Why`) and before the
    /// answer's first token, once per chat or task turn. In a pipeline each stage's note comes
    /// after that stage's `Routed` and before the stage runs. Additive: a reader that does not
    /// know it must skip it.
    Stage(StageNote),
    /// The engine is loading: presence is "working", never a spinner in the app.
    Waiting(Readiness),
    /// Reply text so far.
    TextDelta(String),
    /// Reasoning so far.
    ThoughtDelta(String),
    /// A function call the model made.
    ToolCall(ToolCallPart),
    /// A computer-use action as parsed, for the acting-here glow preview.
    ActionProposed(CuaAction<WindowSpace>),
    /// Tokens spent so far.
    Usage(TokenUsage),
    /// What the recogniser has heard.
    Heard(HeardDelta),
    /// A piece of synthesised speech.
    Spoken(AudioFrameOut),
    /// The turn is over.
    Finished(InferReply),
}

/// What a sink answers to an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    /// Keep going.
    Continue,
    /// End the turn early (barge-in, the user stopped it).
    Stop,
}

/// One stage of the pipeline that answers a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageNote {
    /// What the stage does.
    pub role: StageRole,
    /// Who runs it.
    pub served: ServedBy,
    /// Why that model (the person sees it when `ai.auto.show_reason` is on).
    pub why: Why,
    /// The model's name as a person reads it ("Gemma 4"), from the catalogue entry's label. Absent
    /// when inferd has no label for the model, and in a note from an inferd that predates it: a
    /// payload without it decodes, and a note without it is written without the field, so old
    /// readers see the bytes they always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ModelLabel>,
}

/// A model's display name (the catalogue entry's label), for the footer's "Answered by <name>".
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelLabel(pub String);
