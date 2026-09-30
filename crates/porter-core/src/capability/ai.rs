//! The AI kinds (design/31 §2.3): declared per model, aggregated per account. Feature sets are
//! sets of variants, never flags.

use crate::error::CoreError;
use crate::units::{Count, Dims, Px, Tokens};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A language model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LlmCap {
    /// What it can do beyond plain text in, text out.
    pub features: BTreeSet<LlmFeature>,
    /// The context window.
    pub context: Tokens,
    /// The longest reply.
    pub max_output: Tokens,
    /// The wire format inferd's adapter speaks to it.
    pub wire: LlmWire,
}

/// One language-model feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmFeature {
    /// Multi-turn chat.
    Chat,
    /// Tool (function) calls.
    Tools,
    /// Images in the prompt.
    Vision,
    /// Audio in the prompt.
    AudioIn,
    /// PDF documents in the prompt.
    Pdf,
    /// Replies constrained to a JSON schema.
    StructuredOutput,
    /// Extended reasoning before the reply.
    Reasoning,
    /// Server-side prompt caching.
    PromptCache,
}

/// The request format a model is reached with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmWire {
    /// OpenAI-compatible Chat Completions (also Ollama, llama.cpp, vLLM, OpenRouter).
    ChatCompletions,
    /// OpenAI Responses.
    Responses,
    /// Anthropic Messages.
    Messages,
    /// Gemini generateContent.
    GenerateContent,
    /// Ollama's own API.
    OllamaNative,
}

/// An embedding model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmbedCap {
    /// The vector length.
    pub dims: Dims,
    /// What it embeds.
    pub modalities: BTreeSet<Modality>,
    /// The longest input.
    pub max_input: Tokens,
}

/// A kind of input an embedding model takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Text.
    Text,
    /// Images.
    Image,
}

/// A speech model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpeechCap {
    /// What it does with speech.
    pub modes: BTreeSet<SpeechMode>,
    /// The languages it handles.
    pub languages: LanguageSet,
}

/// One thing a speech model does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeechMode {
    /// Speech to text.
    Stt,
    /// Text to speech.
    Tts,
    /// A live two-way voice session.
    Realtime,
}

/// The languages a speech model handles.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum LanguageSet {
    /// Whatever the user speaks; the provider publishes no list.
    Any,
    /// Only these.
    Listed(BTreeSet<LanguageTag>),
}

/// A BCP 47 language tag (`en`, `zh-Hant-TW`), checked for its character set only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LanguageTag(String);

impl LanguageTag {
    /// The tag written as `text`: 1 to 35 bytes of ASCII letters, digits and `-`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let ok = !text.is_empty()
            && text.len() <= 35
            && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "language tag",
                text: text.to_owned(),
            })
        }
    }
}

impl TryFrom<String> for LanguageTag {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<LanguageTag> for String {
    fn from(tag: LanguageTag) -> String {
        tag.0
    }
}

/// An image generator (a hosted model or a ComfyUI workflow).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageGenCap {
    /// What it does.
    pub modes: BTreeSet<ImageMode>,
    /// The longest side of an image it makes.
    pub max_side: Px,
}

/// One thing an image generator does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMode {
    /// An image from a prompt.
    TextToImage,
    /// A changed image from an image and a prompt.
    Edit,
    /// A masked region repainted.
    Inpaint,
}

/// A reranker.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RerankCap {
    /// The most documents one call ranks.
    pub max_docs: Count,
}
