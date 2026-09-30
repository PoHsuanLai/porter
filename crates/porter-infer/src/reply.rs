//! What inferd answers.

use crate::error::InferRefusal;
use porter_core::{AccountId, Locality, ModelId, Tokens};
use serde::{Deserialize, Serialize};

/// One reply from inferd.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum InferReply {
    /// For `Chat` and `Task`.
    Chat(ChatReply),
    /// For `Embed`.
    Embed(EmbedReply),
    /// Refused, and why; never a silent downgrade.
    Refused(InferRefusal),
}

/// A chat or task answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatReply {
    /// The text (JSON text for `ReplyShape::Json`).
    pub text: String,
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
