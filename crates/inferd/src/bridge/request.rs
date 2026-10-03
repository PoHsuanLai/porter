//! Porter's requests as stoker's turns: `ChatRequest` to `TurnRequest`, `TaskRequest` to a chat,
//! `EmbedRequest` to `EmbedTurn`. A pure mapping over the model's catalog entry (its default
//! sampling and output limit), the descriptors that rode with the frame, and the engine's
//! flavor. The only place the two vocabularies meet on the way in.

use crate::local::LocalModel;
use model_openai_compat::Flavor;
use model_provider as sp;
use porter_core::need::DimsNeed;
use porter_infer as pi;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
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
    /// wrote). A descriptor over [`MAX_ATTACHMENT`] bytes is refused.
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
        pi::ThoughtSeal::Redacted(text) => sp::ThoughtSeal::Redacted(sp::OpaqueText(text.0.clone())),
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

fn shape(shape: &pi::ReplyShape) -> Result<sp::OutputShape, BridgeError> {
    Ok(match shape {
        pi::ReplyShape::Text => sp::OutputShape::Free,
        pi::ReplyShape::Json(schema) => sp::OutputShape::JsonSchema(sp::SchemaText(
            sp::JsonText::new(schema.as_str()).map_err(|_| BridgeError::Unsupported)?,
        )),
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

/// Stoker's `Reasoning` has no "whatever the engine does": an app that leaves it open gets no
/// reasoning, so a short interactive turn does not spend its tokens thinking. An app that wants
/// it asks for `On(effort)`.
fn reasoning(reasoning: pi::Reasoning) -> sp::Reasoning {
    match reasoning {
        pi::Reasoning::EngineDefault | pi::Reasoning::Off => sp::Reasoning::Off,
        pi::Reasoning::On(level) => sp::Reasoning::On(effort(level)),
    }
}

/// What the engine's flavor understands beyond the shared fields.
fn extras(flavor: Option<Flavor>) -> sp::EngineExtras {
    match flavor {
        Some(Flavor::LlamaServer) => sp::EngineExtras::LlamaServer(sp::LlamaExtras {
            cache_prompt: sp::PromptCache::Reuse,
            slot: sp::Knob::Off,
        }),
        _ => sp::EngineExtras::None,
    }
}

/// The catalog's sampling for a turn that does not choose its own: the set for the reasoning
/// mode the turn asks for.
fn default_sampling(model: &LocalModel, reasoning: sp::Reasoning) -> Option<sp::Sampling> {
    model.entry.sampling.map(|defaults| match reasoning {
        sp::Reasoning::Off => defaults.reasoning_off,
        sp::Reasoning::On(_) => defaults.reasoning_on,
    })
}

/// The turn for a chat request. Sampling and the output limit the request leaves open come from
/// the model's catalog entry.
pub fn chat_turn(
    model: &LocalModel,
    request: &pi::ChatRequest,
    frames: &Frames,
) -> Result<sp::TurnRequest, BridgeError> {
    let control = &request.control;
    let reasoning = reasoning(control.reasoning);
    let sampling = match control.sampling {
        pi::Knob::Set(own) => Some(sampling(own)),
        pi::Knob::Off => default_sampling(model, reasoning),
    }
    .ok_or(BridgeError::Unsupported)?;
    let max_output = match control.max_output {
        pi::Knob::Set(tokens) => sp::Tokens(tokens.0),
        pi::Knob::Off => model.caps().map(|caps| caps.max_output).unwrap_or_default(),
    };
    Ok(sp::TurnRequest {
        model: model.name.clone(),
        messages: request
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
            .collect::<Result<_, BridgeError>>()?,
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
        output: shape(&request.shape)?,
        limits: sp::Limits {
            max_output,
            stop: control.stop.clone(),
        },
        sampling,
        reasoning,
        engine: extras(model.flavor),
    })
}

/// The instruction a task kind is carried out under.
fn task_instruction(task: pi::Task) -> &'static str {
    match task {
        pi::Task::Summarise => "Summarise the text the user gives you. Reply with the summary only.",
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

/// A task as a chat turn: its instruction, then the text.
pub fn task_turn(
    model: &LocalModel,
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
        },
    };
    chat_turn(model, &chat, &Frames::default())
}

/// The embedding turns of a request: the model's prefix for the request's role goes before every
/// text, and the texts are cut into batches of the model's limit.
pub fn embed_turns(
    model: &LocalModel,
    request: &pi::EmbedRequest,
) -> Result<Vec<sp::EmbedTurn>, BridgeError> {
    let spec = model.embed.as_ref().ok_or(BridgeError::Unsupported)?;
    let (prefix, role) = match request.role {
        pi::EmbedRole::Query => (&spec.query_prefix, sp::EmbedRole::Query),
        pi::EmbedRole::Document => (&spec.document_prefix, sp::EmbedRole::Document),
    };
    let dims = match request.dims {
        DimsNeed::Any => sp::Knob::Off,
        DimsNeed::Exactly(dims) => sp::Knob::Set(sp::Dims(dims.0)),
    };
    let batches = sp::plan_batches(request.inputs.len(), sp::BatchMax(spec.max_batch));
    Ok(batches
        .into_iter()
        .map(|range| sp::EmbedTurn {
            model: model.name.clone(),
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
