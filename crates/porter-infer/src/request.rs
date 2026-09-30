//! What an app sends: one model whatever the provider; wire formats stay in the adapters.

use porter_core::consent::Usage;
use porter_core::need::DimsNeed;
use porter_core::{DataClass, Tier};
use serde::{Deserialize, Serialize};

/// One request to inferd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum InferRequest {
    /// A chat turn.
    Chat(ChatRequest),
    /// Embeddings.
    Embed(EmbedRequest),
    /// A task above raw chat; the shell uses these, never model ids.
    Task(TaskRequest),
}

/// A chat turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The conversation so far, oldest first.
    pub messages: Vec<ChatMessage>,
    /// Plain text or JSON matching a schema.
    pub shape: ReplyShape,
    /// The tier the app asks for; the user maps tiers to models.
    pub tier: Tier,
    /// The data the messages carry: the routing floor and the grant are per class.
    pub class: DataClass,
    /// Interactive or background.
    pub usage: Usage,
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Who says it.
    pub role: Role,
    /// Its content, in order.
    pub parts: Vec<MessagePart>,
}

/// Who says a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Instructions.
    System,
    /// The person.
    User,
    /// The model.
    Assistant,
}

/// One piece of a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum MessagePart {
    /// Text.
    Text(String),
    /// An image.
    Image(ImagePart),
}

/// An image in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePart {
    /// Its media type (`image/png`).
    pub media_type: String,
    /// Its bytes.
    pub bytes: Vec<u8>,
}

/// The form of the reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ReplyShape {
    /// Free text.
    Text,
    /// JSON matching this JSON Schema (the schema's text).
    Json(String),
}

/// Embeddings for some texts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedRequest {
    /// The texts.
    pub inputs: Vec<String>,
    /// The vector length an existing index needs, or any.
    pub dims: DimsNeed,
    /// The data the texts carry.
    pub class: DataClass,
    /// Interactive or background (indexing).
    pub usage: Usage,
}

/// A task above raw chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    /// Shorten.
    Summarise,
    /// Say it differently.
    Rewrite,
    /// Pull out fields.
    Extract,
    /// Pick a label.
    Classify,
    /// Speech to text.
    Transcribe,
}

/// One task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequest {
    /// The task.
    pub task: Task,
    /// Its input text.
    pub input: String,
    /// The data the input carries.
    pub class: DataClass,
    /// Interactive or background.
    pub usage: Usage,
}
