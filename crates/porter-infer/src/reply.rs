//! What inferd answers.

use crate::control::StopReason;
use crate::cua::CuaStepReply;
use crate::error::{InferRefusal, ModelError};
use crate::scores::OptionScores;
use porter_core::{AccountId, Locality, ModelId, Tokens};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The last event of a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InferReply {
    /// For `Chat` and `Task`.
    Chat(ChatReply),
    /// For `Embed`.
    Embed(EmbedReply),
    /// For `CuaStep`.
    CuaStep(CuaStepReply),
    /// For `Transcribe`.
    Transcribed(TranscribeReply),
    /// For `Speak`.
    Spoke(SpeakReply),
    /// Refused, and why; never a silent downgrade.
    Refused(InferRefusal),
    /// The model call failed.
    Failed(ModelError),
    /// The client cancelled.
    Cancelled,
}

/// A chat or task answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChatReply {
    /// The text (JSON text for `ReplyShape::Json`).
    pub text: String,
    /// The function calls the model made, in order.
    pub tool_calls: Vec<crate::request::ToolCallPart>,
    /// Why it ended.
    pub stop: StopReason,
    /// What it reasoned, when the request allowed reasoning and the engine returned it.
    pub thought: Option<String>,
    /// Tokens spent.
    pub usage: TokenUsage,
    /// Who answered: the "sent to <provider>" indicator reads it.
    pub served: ServedBy,
    /// How likely each declared option was, when the request asked (`ChatControl::scores`) for
    /// a `Choice` and the engine could tell. Absent otherwise, and never a reason for the call to
    /// fail: an engine that reports nothing usable leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scores: Option<OptionScores>,
}

impl ChatReply {
    /// An answer with this text, ended for this reason, that cost `usage` and was given by
    /// `served`: no function calls, no reasoning and no scores until the `with_*` methods say so.
    pub fn new(text: String, stop: StopReason, usage: TokenUsage, served: ServedBy) -> Self {
        Self {
            text,
            tool_calls: Vec::new(),
            stop,
            thought: None,
            usage,
            served,
            scores: None,
        }
    }

    /// The same answer with the function calls the model made.
    pub fn with_tool_calls(mut self, tool_calls: Vec<crate::request::ToolCallPart>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// The same answer with what the model reasoned.
    pub fn with_thought(mut self, thought: String) -> Self {
        self.thought = Some(thought);
        self
    }

    /// The same answer with how likely each declared option was.
    pub fn with_scores(mut self, scores: OptionScores) -> Self {
        self.scores = Some(scores);
        self
    }
}

/// Embeddings, one per input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EmbedReply {
    /// The vectors, in input order.
    pub vectors: Vec<EmbedVector>,
    /// Tokens spent.
    pub usage: TokenUsage,
    /// Who answered.
    pub served: ServedBy,
}

impl EmbedReply {
    /// These vectors, in input order, that cost `usage` and were given by `served`.
    pub fn new(vectors: Vec<EmbedVector>, usage: TokenUsage, served: ServedBy) -> Self {
        Self {
            vectors,
            usage,
            served,
        }
    }
}

/// One embedding. It holds floats because embeddings are floats end to end (every model emits
/// and every index compares them as such); so it is `PartialEq` without `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbedVector(pub Vec<f32>);

/// A finished transcript.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TranscribeReply {
    /// Everything that was said.
    pub text: String,
    /// How much audio was transcribed, in milliseconds.
    pub audio_ms: u32,
    /// Who answered.
    pub served: ServedBy,
}

impl TranscribeReply {
    /// A transcript of `audio_ms` milliseconds of audio, given by `served`.
    pub fn new(text: String, audio_ms: u32, served: ServedBy) -> Self {
        Self {
            text,
            audio_ms,
            served,
        }
    }
}

// The text is what the person said: Debug shows its length only.
impl fmt::Debug for TranscribeReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranscribeReply")
            .field("text", &format_args!("<{} bytes>", self.text.len()))
            .field("audio_ms", &self.audio_ms)
            .field("served", &self.served)
            .finish()
    }
}

/// Speech was synthesised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SpeakReply {
    /// How much audio was produced, in milliseconds.
    pub audio_ms: u32,
    /// Who answered.
    pub served: ServedBy,
}

impl SpeakReply {
    /// Speech of `audio_ms` milliseconds, given by `served`.
    pub fn new(audio_ms: u32, served: ServedBy) -> Self {
        Self { audio_ms, served }
    }
}

/// Tokens in and out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Prompt tokens.
    pub input: Tokens,
    /// Reply tokens.
    pub output: Tokens,
    /// Prompt tokens the engine served from its cache; a share of `input`.
    pub cached: Tokens,
}

/// The account and model that answered, and where it ran.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServedBy {
    /// The account.
    pub account: AccountId,
    /// The model.
    pub model: ModelId,
    /// Where it ran.
    pub locality: Locality,
}
