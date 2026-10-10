//! Porter's requests as stoker's turns: `ChatRequest` to `TurnRequest`, `TaskRequest` to a chat,
//! `EmbedRequest` to `EmbedTurn`. A pure mapping over the model's catalog entry (its default
//! sampling and output limit), the descriptors that rode with the frame, and the engine's
//! flavor. The only place the two vocabularies meet on the way in.
//!
//! The mapping is over a [`Target`] (the name the server knows the model by, where an open
//! sampling comes from, the reply limit, the dialect), so it serves a model inferd supervises, a
//! hosted one and an engine an app points at alike; what builds a target from a model book is the
//! caller's.

use model_catalog::SamplingDefaults;
use model_openai_compat::Flavor;
use model_provider as sp;
use model_provider::EmbedCaps;
use porter_core::need::DimsNeed;
use porter_infer as pi;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::fd::OwnedFd;

/// The most bytes one attached image may have: a 16k by 16k frame at four bytes a pixel.
pub const MAX_ATTACHMENT: u64 = 1 << 30;

/// Why a request could not be turned into a model turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// A name, schema or argument text is malformed, or an image type has no encoder.
    #[error("the request cannot be expressed to the engine")]
    Unsupported,
    /// A descriptor could not be read, is too large, or the request names one that is not there.
    #[error("an attached image could not be read")]
    Attachment,
}

/// The bytes of the descriptors that came with one frame, by attach index.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frames(Vec<Vec<u8>>);

impl Frames {
    /// Reads every descriptor from the start (the sender's offset is at the end of what it
    /// wrote). A descriptor over [`MAX_ATTACHMENT`] bytes is refused. Descriptors ride on frames
    /// on Unix only: where there are none, a request names no attachment and `Frames::default()`
    /// is all it needs.
    #[cfg(unix)]
    pub fn read(fds: Vec<OwnedFd>) -> Result<Self, BridgeError> {
        fds.into_iter()
            .map(|fd| {
                let mut file = File::from(fd);
                let size = file.metadata().map_err(|_| BridgeError::Attachment)?.len();
                if size > MAX_ATTACHMENT {
                    return Err(BridgeError::Attachment);
                }
                file.seek(SeekFrom::Start(0))
                    .map_err(|_| BridgeError::Attachment)?;
                let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
                file.read_to_end(&mut bytes)
                    .map_err(|_| BridgeError::Attachment)?;
                Ok(bytes)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// The bytes behind a source: inline ones as they are, attached ones by index.
    pub fn resolve(&self, source: &pi::ImageSource) -> Result<Vec<u8>, BridgeError> {
        match source {
            pi::ImageSource::Inline(bytes) => Ok(bytes.0.clone()),
            pi::ImageSource::Attached(pi::AttachIndex(index)) => self
                .0
                .get(usize::from(*index))
                .cloned()
                .ok_or(BridgeError::Attachment),
        }
    }
}

/// The media type of an image part.
fn media_of(media_type: &str) -> Result<vision_prep::MediaType, BridgeError> {
    match media_type {
        "image/png" => Ok(vision_prep::MediaType::Png),
        "image/jpeg" => Ok(vision_prep::MediaType::Jpeg),
        _ => Err(BridgeError::Unsupported),
    }
}

/// An encoded image of this media type, ready for a turn.
pub fn image_input(media: vision_prep::MediaType, bytes: Vec<u8>) -> sp::ImageInput {
    sp::ImageInput {
        media,
        bytes: sp::ImageBytes(bytes),
        detail: sp::ImageDetail::Auto,
    }
}

fn tool_name(name: &pi::ToolName) -> Result<sp::ToolName, BridgeError> {
    sp::ToolName::new(name.as_str()).map_err(|_| BridgeError::Unsupported)
}

fn json(text: &pi::JsonText) -> Result<sp::JsonText, BridgeError> {
    sp::JsonText::new(text.as_str()).map_err(|_| BridgeError::Unsupported)
}

fn seal(seal: &pi::ThoughtSeal) -> sp::ThoughtSeal {
    match seal {
        pi::ThoughtSeal::None => sp::ThoughtSeal::None,
        pi::ThoughtSeal::Signed(text) => sp::ThoughtSeal::Signed(sp::SignatureText(text.0.clone())),
        pi::ThoughtSeal::Redacted(text) => {
            sp::ThoughtSeal::Redacted(sp::OpaqueText(text.0.clone()))
        }
    }
}

fn part(one: &pi::MessagePart, frames: &Frames) -> Result<sp::Part, BridgeError> {
    Ok(match one {
        pi::MessagePart::Text(text) => sp::Part::Text(text.clone()),
        pi::MessagePart::Image(image) => sp::Part::Image(image_input(
            media_of(&image.media_type)?,
            frames.resolve(&image.source)?,
        )),
        pi::MessagePart::Thought(thought) => sp::Part::Thought {
            text: thought.text.clone(),
            seal: seal(&thought.seal),
        },
        pi::MessagePart::ToolCall(call) => sp::Part::ToolCall(sp::ToolCall {
            id: sp::ToolCallId(call.id.0.clone()),
            name: tool_name(&call.name)?,
            input: json(&call.args)?,
        }),
        pi::MessagePart::ToolResult(result) => sp::Part::ToolResult(sp::ToolResult {
            id: sp::ToolCallId(result.id.0.clone()),
            status: match result.status {
                pi::ToolStatus::Ok => sp::ToolStatus::Ok,
                pi::ToolStatus::Error => sp::ToolStatus::Error,
            },
            parts: result
                .parts
                .iter()
                .map(|inner| part(inner, frames))
                .collect::<Result<_, _>>()?,
        }),
    })
}

fn role(role: pi::Role) -> sp::Role {
    match role {
        pi::Role::System => sp::Role::System,
        pi::Role::User => sp::Role::User,
        pi::Role::Assistant => sp::Role::Assistant,
    }
}

fn shape(shape: &pi::ReplyShape, json: JsonReply) -> Result<sp::OutputShape, BridgeError> {
    Ok(match shape {
        pi::ReplyShape::Text => sp::OutputShape::Free,
        pi::ReplyShape::Json(schema) => {
            let schema =
                sp::JsonText::new(schema.as_str()).map_err(|_| BridgeError::Unsupported)?;
            match json {
                JsonReply::Schema => sp::OutputShape::JsonSchema(sp::SchemaText(schema)),
                JsonReply::ObjectOnly => sp::OutputShape::JsonObject,
            }
        }
        pi::ReplyShape::Choice(choices) => sp::OutputShape::Choice(choices.clone()),
    })
}

fn tool_choice(choice: &pi::ToolChoice) -> Result<sp::ToolChoice, BridgeError> {
    Ok(match choice {
        pi::ToolChoice::Auto => sp::ToolChoice::Auto,
        pi::ToolChoice::Never => sp::ToolChoice::Never,
        pi::ToolChoice::Required => sp::ToolChoice::Required,
        pi::ToolChoice::Named(name) => sp::ToolChoice::Named(tool_name(name)?),
    })
}

fn milli(permille: porter_core::Permille) -> sp::Milli {
    sp::Milli(u16::try_from(permille.0).unwrap_or(u16::MAX))
}

fn knob<T, U>(knob: pi::Knob<T>, map: impl Fn(T) -> U) -> sp::Knob<U> {
    match knob {
        pi::Knob::Off => sp::Knob::Off,
        pi::Knob::Set(value) => sp::Knob::Set(map(value)),
    }
}

fn sampling(sampling: pi::Sampling) -> sp::Sampling {
    sp::Sampling {
        temperature: milli(sampling.temperature),
        top_p: knob(sampling.top_p, milli),
        top_k: knob(sampling.top_k, |count| sp::Count(count.0)),
        min_p: knob(sampling.min_p, milli),
        repeat_penalty: sp::Knob::Off,
        seed: knob(sampling.seed, |seed| sp::Seed(seed.0)),
    }
}

fn effort(effort: pi::Effort) -> sp::Effort {
    match effort {
        pi::Effort::Low => sp::Effort::Low,
        pi::Effort::Medium => sp::Effort::Medium,
        pi::Effort::High => sp::Effort::High,
    }
}

/// Reasoning as the app asked: `EngineDefault` stays `EngineDefault` (the codec then sends no
/// reasoning field, and the catalog's `reasoning_default` names the sampling that goes with the
/// engine's own choice).
fn reasoning(reasoning: pi::Reasoning) -> sp::Reasoning {
    match reasoning {
        pi::Reasoning::EngineDefault => sp::Reasoning::EngineDefault,
        pi::Reasoning::Off => sp::Reasoning::Off,
        pi::Reasoning::On(level) => sp::Reasoning::On(effort(level)),
    }
}

/// What the engine's flavor understands beyond the shared fields.
pub fn extras(flavor: Option<Flavor>) -> sp::EngineExtras {
    match flavor {
        Some(Flavor::LlamaServer) => sp::EngineExtras::LlamaServer(sp::LlamaExtras {
            cache_prompt: sp::PromptCache::Reuse,
            slot: sp::Knob::Off,
        }),
        _ => sp::EngineExtras::None,
    }
}

/// What a turn is asked of: the name the server knows the model by, what the turn takes from the
/// model's entry when the request leaves it open, and the dialect of the server.
#[derive(Debug, Clone)]
pub struct Target {
    /// The name in the request's `model` field.
    pub name: sp::ModelName,
    /// Where a sampling the request leaves open comes from.
    pub sampling: DefaultSampling,
    /// The reply limit a request leaves open.
    pub max_output: sp::Tokens,
    /// The engine's flavor, when it has one.
    pub flavor: Option<Flavor>,
    /// How the model takes a reply of a JSON shape (from its entry's output constraints).
    pub json: JsonReply,
}

/// How a model takes a reply of a JSON shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JsonReply {
    /// The engine takes the schema (also what a model whose constraints are not known gets).
    #[default]
    Schema,
    /// The engine takes only "one JSON object": the schema travels in the prompt instead, and
    /// whoever reads the reply checks it.
    ObjectOnly,
}

impl JsonReply {
    /// What a model's output constraints say: `ObjectOnly` when it takes a JSON object and no
    /// schema.
    pub fn of(output: &std::collections::BTreeSet<sp::Constraint>) -> Self {
        if output.contains(&sp::Constraint::JsonObject)
            && !output.contains(&sp::Constraint::JsonSchema)
        {
            Self::ObjectOnly
        } else {
            Self::Schema
        }
    }
}

/// Where a sampling the request leaves open comes from.
#[derive(Debug, Clone, Copy)]
pub enum DefaultSampling {
    /// The entry's table for the reasoning mode asked (none written is a request that cannot be
    /// expressed).
    Entry(Option<SamplingDefaults>),
    /// The provider's own defaults: the turn carries a placeholder and whoever sends it leaves the
    /// field out (a hosted model's entry writes no sampling).
    Provider,
}

/// The sampling a turn carries when the provider's defaults apply: never sent as it stands.
const PROVIDER_DEFAULT: sp::Sampling = sp::Sampling {
    temperature: sp::Milli(1000),
    top_p: sp::Knob::Off,
    top_k: sp::Knob::Off,
    min_p: sp::Knob::Off,
    repeat_penalty: sp::Knob::Off,
    seed: sp::Knob::Off,
};

fn default_sampling(target: &Target, reasoning: sp::Reasoning) -> Option<sp::Sampling> {
    match target.sampling {
        DefaultSampling::Entry(defaults) => {
            defaults.map(|defaults| *defaults.for_reasoning(reasoning))
        }
        DefaultSampling::Provider => Some(PROVIDER_DEFAULT),
    }
}

/// The turn for a chat request on `target`. Sampling and the output limit the request leaves open
/// come from the target.
pub fn chat_turn_for(
    target: &Target,
    request: &pi::ChatRequest,
    frames: &Frames,
) -> Result<sp::TurnRequest, BridgeError> {
    let control = &request.control;
    let reasoning = reasoning(control.reasoning);
    let sampling = match control.sampling {
        pi::Knob::Set(own) => Some(sampling(own)),
        pi::Knob::Off => default_sampling(target, reasoning),
    }
    .ok_or(BridgeError::Unsupported)?;
    let max_output = match control.max_output {
        pi::Knob::Set(tokens) => sp::Tokens(tokens.0),
        pi::Knob::Off => target.max_output,
    };
    let output = shape(&request.shape, target.json)?;
    let choice_scores = crate::scores::choice_scores(control, &output);
    let mut messages: Vec<sp::Message> = request
        .messages
        .iter()
        .map(|message| {
            Ok(sp::Message {
                role: role(message.role),
                parts: message
                    .parts
                    .iter()
                    .map(|one| part(one, frames))
                    .collect::<Result<_, BridgeError>>()?,
            })
        })
        .collect::<Result<_, BridgeError>>()?;
    if let (sp::OutputShape::JsonObject, pi::ReplyShape::Json(schema)) = (&output, &request.shape) {
        schema_in_prompt(&mut messages, schema);
    }
    Ok(sp::TurnRequest {
        model: target.name.clone(),
        messages,
        tools: request
            .tools
            .iter()
            .map(|tool| {
                Ok(sp::ToolSpec::Function {
                    name: tool_name(&tool.name)?,
                    description: tool.description.clone(),
                    parameters: sp::SchemaText(json(&tool.params.0)?),
                })
            })
            .collect::<Result<_, BridgeError>>()?,
        tool_choice: tool_choice(&control.tool_choice)?,
        tool_calls: match control.tool_calls {
            pi::ToolParallelism::One => sp::ToolParallelism::One,
            pi::ToolParallelism::Many => sp::ToolParallelism::Many,
        },
        output,
        limits: sp::Limits {
            max_output,
            stop: control.stop.clone(),
        },
        sampling,
        reasoning,
        choice_scores,
        engine: extras(target.flavor),
    })
}

/// The schema as system text, for a model that takes only a JSON object: appended to the first
/// system message, or a new one in front.
fn schema_in_prompt(messages: &mut Vec<sp::Message>, schema: &str) {
    let text = sp::Part::Text(format!(
        "Reply with one JSON object that matches this JSON schema, and nothing else:\n{schema}"
    ));
    match messages.first_mut() {
        Some(first) if first.role == sp::Role::System => first.parts.push(text),
        _ => messages.insert(
            0,
            sp::Message {
                role: sp::Role::System,
                parts: vec![text],
            },
        ),
    }
}

/// The instruction a task kind is carried out under.
fn task_instruction(task: pi::Task) -> &'static str {
    match task {
        pi::Task::Summarise => {
            "Summarise the text the user gives you. Reply with the summary only."
        }
        pi::Task::Rewrite => {
            "Rewrite the text the user gives you so it reads better, keeping its meaning. Reply with the rewritten text only."
        }
        pi::Task::Extract => {
            "Extract the facts, names, dates and amounts from the text the user gives you. Reply with a plain list, one per line."
        }
        pi::Task::Classify => {
            "Classify the text the user gives you in one or two words. Reply with the label only."
        }
    }
}

/// A task as a chat turn on `target`: its instruction, then the text.
pub fn task_turn_for(
    target: &Target,
    request: &pi::TaskRequest,
    tier: porter_core::Tier,
) -> Result<sp::TurnRequest, BridgeError> {
    let chat = pi::ChatRequest {
        messages: vec![
            pi::ChatMessage {
                role: pi::Role::System,
                parts: vec![pi::MessagePart::Text(task_instruction(request.task).into())],
            },
            pi::ChatMessage {
                role: pi::Role::User,
                parts: vec![pi::MessagePart::Text(request.input.clone())],
            },
        ],
        shape: pi::ReplyShape::Text,
        tier,
        class: request.class,
        usage: request.usage,
        tools: Vec::new(),
        control: pi::ChatControl {
            tool_choice: pi::ToolChoice::Never,
            tool_calls: pi::ToolParallelism::One,
            max_output: pi::Knob::Off,
            reasoning: pi::Reasoning::Off,
            sampling: pi::Knob::Off,
            stop: Vec::new(),
            scores: pi::Knob::Off,
        },
    };
    chat_turn_for(target, &chat, &Frames::default())
}

/// The embedding turns of a request: the model's prefix for the request's role goes before every
/// text, and the texts are cut into batches of the model's limit.
pub fn embed_turns_for(
    name: &sp::ModelName,
    embed: &EmbedCaps,
    request: &pi::EmbedRequest,
) -> Result<Vec<sp::EmbedTurn>, BridgeError> {
    let (prefix, role) = match request.role {
        pi::EmbedRole::Query => (&embed.prompts.query.0, sp::EmbedRole::Query),
        pi::EmbedRole::Document => (&embed.prompts.document.0, sp::EmbedRole::Document),
    };
    let dims = match request.dims {
        DimsNeed::Any => sp::Knob::Off,
        DimsNeed::Exactly(dims) => sp::Knob::Set(sp::Dims(dims.0)),
    };
    let batches = sp::plan_batches(request.inputs.len(), embed.max_batch);
    Ok(batches
        .into_iter()
        .map(|range| sp::EmbedTurn {
            model: name.clone(),
            inputs: request.inputs[range]
                .iter()
                .map(|text| format!("{prefix}{text}"))
                .collect(),
            role,
            dims,
        })
        .collect())
}

#[cfg(test)]
mod tests;
