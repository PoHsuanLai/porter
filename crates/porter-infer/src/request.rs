//! What an app sends: one model whatever the provider; wire formats stay in the adapters.
//! Streaming events are in `event`, the computer-use step in `cua`, speech in `speech`.

use crate::cua::{CuaBegin, CuaStepRequest};
use crate::ids::{AttachIndex, Base64Bytes, JsonSchemaText, JsonText, ToolCallId, ToolName};
use crate::speech::{SpeakRequest, TranscribeBegin};
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
    /// Opens a computer-use run on this session.
    CuaBegin(CuaBegin),
    /// One computer-use step: a frame in, actions out.
    CuaStep(CuaStepRequest),
    /// Speech to text; audio follows as `ClientFrame::Audio` frames.
    Transcribe(TranscribeBegin),
    /// Text to speech; audio comes back as `InferEvent::Spoken` events.
    Speak(SpeakRequest),
}

/// Which kind of request a session carries, as the session machine tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// [`InferRequest::Chat`].
    Chat,
    /// [`InferRequest::Embed`].
    Embed,
    /// [`InferRequest::Task`].
    Task,
    /// [`InferRequest::CuaBegin`].
    CuaBegin,
    /// [`InferRequest::CuaStep`].
    CuaStep,
    /// [`InferRequest::Transcribe`].
    Transcribe,
    /// [`InferRequest::Speak`].
    Speak,
}

impl InferRequest {
    /// Which kind this is.
    pub fn kind(&self) -> RequestKind {
        match self {
            InferRequest::Chat(_) => RequestKind::Chat,
            InferRequest::Embed(_) => RequestKind::Embed,
            InferRequest::Task(_) => RequestKind::Task,
            InferRequest::CuaBegin(_) => RequestKind::CuaBegin,
            InferRequest::CuaStep(_) => RequestKind::CuaStep,
            InferRequest::Transcribe(_) => RequestKind::Transcribe,
            InferRequest::Speak(_) => RequestKind::Speak,
        }
    }
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
    /// The functions the model may call (from the router's action declarations).
    pub tools: Vec<ToolDecl>,
}

/// One function a model may call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDecl {
    /// Its name.
    pub name: ToolName,
    /// What it does, for the model.
    pub description: String,
    /// Its parameters, as a JSON Schema.
    pub params: JsonSchemaText,
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
    /// A function call the model made.
    ToolCall(ToolCallPart),
    /// What a function returned.
    ToolResult(ToolResultPart),
}

/// A function call in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallPart {
    /// The model's id for the call, echoed by its result.
    pub id: ToolCallId,
    /// The function.
    pub name: ToolName,
    /// Its arguments, as JSON text.
    pub args: JsonText,
}

/// A function's result in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultPart {
    /// The call it answers.
    pub id: ToolCallId,
    /// Whether it worked.
    pub status: ToolStatus,
    /// What came back.
    pub parts: Vec<MessagePart>,
}

/// Whether a function call worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// It ran.
    Ok,
    /// It failed or was refused.
    Error,
}

/// An image in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePart {
    /// Its media type (`image/png`).
    pub media_type: String,
    /// Where its bytes are.
    pub source: ImageSource,
}

/// Where an image's bytes are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum ImageSource {
    /// In the frame, as base64 text (small images; about a third larger than raw).
    Inline(Base64Bytes),
    /// In a memfd received with SCM_RIGHTS on the `Open` fd, never copied through JSON.
    Attached(AttachIndex),
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

/// A task above raw chat. Speech to text is not here: its input is audio, not text (see
/// [`InferRequest::Transcribe`]).
///
/// ```compile_fail
/// let _ = porter_infer::Task::Transcribe;
/// ```
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
