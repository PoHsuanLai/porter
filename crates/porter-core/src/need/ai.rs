//! Needs for the AI kinds. Sets are subsets the offer must contain; numbers are minimums.
//!
//! Each need grows by adding a minimum, so each is `#[non_exhaustive]` and built with `new`.

use crate::capability::{CuaEnv, ImageMode, LlmFeature, Modality, SpeechMode};
use crate::units::{Count, Dims, Px, Tokens};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A language model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LlmNeed {
    /// Features it must have.
    pub features: BTreeSet<LlmFeature>,
    /// At least this context window.
    pub context: Tokens,
}

impl LlmNeed {
    /// A language model with these features and at least this context window.
    pub fn new(features: BTreeSet<LlmFeature>, context: Tokens) -> Self {
        Self { features, context }
    }
}

/// An embedding model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EmbedNeed {
    /// The vector length an index was built with, or any.
    pub dims: DimsNeed,
    /// Inputs it must embed.
    pub modalities: BTreeSet<Modality>,
}

impl EmbedNeed {
    /// An embedding model with this vector length that embeds these inputs.
    pub fn new(dims: DimsNeed, modalities: BTreeSet<Modality>) -> Self {
        Self { dims, modalities }
    }
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
#[non_exhaustive]
pub struct SpeechNeed {
    /// Modes it must have.
    pub modes: BTreeSet<SpeechMode>,
}

impl SpeechNeed {
    /// A speech model with these modes.
    pub fn new(modes: BTreeSet<SpeechMode>) -> Self {
        Self { modes }
    }
}

/// An image generator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageGenNeed {
    /// Modes it must have.
    pub modes: BTreeSet<ImageMode>,
    /// Images at least this large on their longest side.
    pub max_side: Px,
}

impl ImageGenNeed {
    /// An image generator with these modes and images at least this large.
    pub fn new(modes: BTreeSet<ImageMode>, max_side: Px) -> Self {
        Self { modes, max_side }
    }
}

/// A reranker.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RerankNeed {
    /// At least this many documents per call.
    pub max_docs: Count,
}

impl RerankNeed {
    /// A reranker that takes at least this many documents per call.
    pub fn new(max_docs: Count) -> Self {
        Self { max_docs }
    }
}

/// A computer-use model. Only the environments are asked for; protocol fields (batching, wire,
/// image size) are the engine's business.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CuaNeed {
    /// Environments it must operate.
    pub environments: BTreeSet<CuaEnv>,
}

impl CuaNeed {
    /// A computer-use model that operates these environments.
    pub fn new(environments: BTreeSet<CuaEnv>) -> Self {
        Self { environments }
    }
}
