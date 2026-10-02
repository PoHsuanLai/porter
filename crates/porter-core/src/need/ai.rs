//! Needs for the AI kinds. Sets are subsets the offer must contain; numbers are minimums.

use crate::capability::{CuaEnv, ImageMode, LlmFeature, Modality, SpeechMode};
use crate::units::{Count, Dims, Px, Tokens};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A language model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LlmNeed {
    /// Features it must have.
    pub features: BTreeSet<LlmFeature>,
    /// At least this context window.
    pub context: Tokens,
}

/// An embedding model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmbedNeed {
    /// The vector length an index was built with, or any.
    pub dims: DimsNeed,
    /// Inputs it must embed.
    pub modalities: BTreeSet<Modality>,
}

/// Which vector length an embedding need accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum DimsNeed {
    /// Any length (a fresh index).
    Any,
    /// Exactly this length (an existing index).
    Exactly(Dims),
}

/// A speech model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpeechNeed {
    /// Modes it must have.
    pub modes: BTreeSet<SpeechMode>,
}

/// An image generator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageGenNeed {
    /// Modes it must have.
    pub modes: BTreeSet<ImageMode>,
    /// Images at least this large on their longest side.
    pub max_side: Px,
}

/// A reranker.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RerankNeed {
    /// At least this many documents per call.
    pub max_docs: Count,
}

/// A computer-use model. Only the environments are asked for; protocol fields (batching, wire,
/// image size) are the engine's business.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CuaNeed {
    /// Environments it must operate.
    pub environments: BTreeSet<CuaEnv>,
}
