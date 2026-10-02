//! What inferd answers.

use crate::cua::CuaStepReply;
use crate::error::{InferRefusal, ModelError};
use porter_core::{AccountId, Locality, ModelId, Tokens};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The last event of a turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
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
pub struct ChatReply {
    /// The text (JSON text for `ReplyShape::Json`).
    pub text: String,
    /// The function calls the model made, in order.
    pub tool_calls: Vec<crate::request::ToolCallPart>,
    /// Tokens spent.
    pub usage: TokenUsage,
    /// Who answered: the "sent to <provider>" indicator reads it.
    pub served: ServedBy,
}

/// Embeddings, one per input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedReply {
    /// The vectors, in input order.
    pub vectors: Vec<EmbedVector>,
    /// Tokens spent.
    pub usage: TokenUsage,
    /// Who answered.
    pub served: ServedBy,
}

/// One embedding. It holds floats because embeddings are floats end to end (every model emits
/// and every index compares them as such); so it is `PartialEq` without `Eq`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbedVector(pub Vec<f32>);

/// A finished transcript.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscribeReply {
    /// Everything that was said.
    pub text: String,
    /// How much audio was transcribed, in milliseconds.
    pub audio_ms: u32,
    /// Who answered.
    pub served: ServedBy,
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
pub struct SpeakReply {
    /// How much audio was produced, in milliseconds.
    pub audio_ms: u32,
    /// Who answered.
    pub served: ServedBy,
}

/// Tokens in and out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Prompt tokens.
    pub input: Tokens,
    /// Reply tokens.
    pub output: Tokens,
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
