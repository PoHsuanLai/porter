//! What an app sends: one model whatever the provider; wire formats stay in the adapters.
//! Streaming events are in `event`, the computer-use step in `cua`, speech in `speech`.

use crate::control::{ChatControl, ThoughtSeal};
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
#[non_exhaustive]
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
#[non_exhaustive]
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

    /// How many descriptors must ride with the frame that carries this request: one more than
    /// the highest `AttachIndex` it names, so the receiver takes exactly that many from the
    /// fd queue (a frame naming none takes none).
    pub fn attachments(&self) -> usize {
        let sources: Vec<&ImageSource> = match self {
            InferRequest::Chat(chat) => chat
                .messages
                .iter()
                .flat_map(|message| message.parts.iter())
                .flat_map(MessagePart::images)
                .collect(),
            InferRequest::CuaStep(step) => vec![&step.frame.source],
            _ => vec![],
        };
        sources
            .into_iter()
            .filter_map(|source| match source {
                ImageSource::Attached(AttachIndex(index)) => Some(usize::from(*index) + 1),
                ImageSource::Inline(_) => None,
            })
            .max()
            .unwrap_or(0)
    }
}

impl MessagePart {
    /// The image sources in this part, looking inside a tool result.
    fn images(&self) -> Vec<&ImageSource> {
        match self {
            MessagePart::Image(image) => vec![&image.source],
            MessagePart::ToolResult(result) => {
                result.parts.iter().flat_map(MessagePart::images).collect()
            }
            MessagePart::Text(_) | MessagePart::ToolCall(_) | MessagePart::Thought(_) => vec![],
        }
    }
}

/// A chat turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// Tool choice, parallelism, the output limit, reasoning, sampling and stop strings.
    pub control: ChatControl,
}

impl ChatRequest {
    /// A chat turn over `messages` at `tier`, for data of `class`: a plain text reply, no
    /// functions, and the default controls ([`ChatControl::new`]).
    pub fn new(messages: Vec<ChatMessage>, tier: Tier, class: DataClass, usage: Usage) -> Self {
        Self {
            messages,
            shape: ReplyShape::Text,
            tier,
            class,
            usage,
            tools: Vec::new(),
            control: ChatControl::new(),
        }
    }

    /// The same turn asking for this form of reply.
    pub fn with_shape(mut self, shape: ReplyShape) -> Self {
        self.shape = shape;
        self
    }

    /// The same turn offering these functions.
    pub fn with_tools(mut self, tools: Vec<ToolDecl>) -> Self {
        self.tools = tools;
        self
    }

    /// The same turn with these controls.
    pub fn with_control(mut self, control: ChatControl) -> Self {
        self.control = control;
        self
    }
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
#[non_exhaustive]
pub enum MessagePart {
    /// Text.
    Text(String),
    /// An image.
    Image(ImagePart),
    /// A function call the model made.
    ToolCall(ToolCallPart),
    /// What a function returned.
    ToolResult(ToolResultPart),
    /// Reasoning the model produced, handed back with its seal. Apps never construct one;
    /// inferd keeps it across tool turns.
    Thought(ThoughtPart),
}

/// A thought in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThoughtPart {
    /// The reasoning text (empty when the provider redacted it).
    pub text: String,
    /// What the provider attached to hand back with it.
    pub seal: ThoughtSeal,
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
#[non_exhaustive]
pub enum ImageSource {
    /// In the frame, as base64 text (small images; about a third larger than raw).
    Inline(Base64Bytes),
    /// In a memfd received with SCM_RIGHTS on the `Open` fd, never copied through JSON.
    Attached(AttachIndex),
}

/// The form of the reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReplyShape {
    /// Free text.
    Text,
    /// JSON matching this JSON Schema (the schema's text).
    Json(String),
    /// Exactly one of these strings (a reviewer's verdict).
    Choice(Vec<String>),
}

/// Embeddings for some texts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EmbedRequest {
    /// The texts.
    pub inputs: Vec<String>,
    /// Whether they are search queries or passages to index: asymmetric models embed the two
    /// differently, and inferd puts the model's prefix in front.
    pub role: EmbedRole,
    /// The vector length an existing index needs, or any.
    pub dims: DimsNeed,
    /// The data the texts carry.
    pub class: DataClass,
    /// Interactive or background (indexing).
    pub usage: Usage,
}

impl EmbedRequest {
    /// Embeddings for `inputs`, as queries or passages (`role`), of this vector length, for data
    /// of `class`.
    pub fn new(
        inputs: Vec<String>,
        role: EmbedRole,
        dims: DimsNeed,
        class: DataClass,
        usage: Usage,
    ) -> Self {
        Self {
            inputs,
            role,
            dims,
            class,
            usage,
        }
    }
}

/// What an embedded text is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbedRole {
    /// A search query.
    Query,
    /// A passage that is indexed.
    Document,
}

/// A task above raw chat. Speech to text is not here: its input is audio, not text (see
/// [`InferRequest::Transcribe`]).
///
/// ```compile_fail
/// let _ = porter_infer::Task::Transcribe;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
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
#[non_exhaustive]
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

impl TaskRequest {
    /// `task` over `input`, which carries data of `class`.
    pub fn new(task: Task, input: String, class: DataClass, usage: Usage) -> Self {
        Self {
            task,
            input,
            class,
            usage,
        }
    }
}
