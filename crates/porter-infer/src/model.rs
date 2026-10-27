//! The seam a model is run through, wherever it lives: a model of this computer, an engine the
//! person attached, one of their other computers, or a cloud account. inferd's runner starts every
//! chat, task and embedding turn through it ([`answer`]), and the fake tests drive.
//!
//! Events go to a [`ChatSink`] callback rather than a `Stream`: a turn is one future that pushes
//! as it runs, so dropping it cancels, and the sink can end the turn early by returning
//! [`Flow::Stop`].

use crate::error::{InferRefusal, ModelError};
use crate::event::{Flow, InferEvent};
use crate::reply::{ChatReply, EmbedReply, InferReply};
use crate::request::{ChatRequest, EmbedRequest, InferRequest, TaskRequest};
use porter_core::{AccountId, Billing, Capability, Locality, ModelId};
use std::future::Future;

/// What routing knows about one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCard {
    /// The account that serves it.
    pub account: AccountId,
    /// Its id.
    pub model: ModelId,
    /// Where it runs.
    pub locality: Locality,
    /// What it costs.
    pub billing: Billing,
    /// Its effective AI capabilities.
    pub capabilities: Vec<Capability>,
}

/// Where a turn's events go as they happen.
pub trait ChatSink: Send {
    /// One event; `Flow::Stop` ends the turn early.
    fn event(&mut self, event: InferEvent) -> Flow;
}

/// Why a turn on a model gave no reply: it was not run, or it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TurnError {
    /// The request was not run (a cap, a missing grant, a kind of request the model does not take).
    Refused(InferRefusal),
    /// The model was asked and failed.
    Failed(ModelError),
}

impl From<InferRefusal> for TurnError {
    fn from(refusal: InferRefusal) -> Self {
        TurnError::Refused(refusal)
    }
}

impl From<ModelError> for TurnError {
    fn from(error: ModelError) -> Self {
        TurnError::Failed(error)
    }
}

impl From<TurnError> for InferReply {
    fn from(error: TurnError) -> Self {
        match error {
            TurnError::Refused(refusal) => InferReply::Refused(refusal),
            TurnError::Failed(error) => InferReply::Failed(error),
        }
    }
}

/// One model behind one account, wherever it runs. An implementation may be made for one request
/// and hold what that request brings, such as its attachments.
///
/// Cancellation is dropping the future: it closes the engine's stream.
pub trait Model: Send + Sync {
    /// Its card.
    fn card(&self) -> &ModelCard;

    /// Runs a chat turn, pushing deltas and tool calls into `sink`.
    fn chat(
        &self,
        request: &ChatRequest,
        sink: &mut impl ChatSink,
    ) -> impl Future<Output = Result<ChatReply, TurnError>> + Send;

    /// Runs a task above raw chat, pushing deltas into `sink`.
    fn task(
        &self,
        request: &TaskRequest,
        sink: &mut impl ChatSink,
    ) -> impl Future<Output = Result<ChatReply, TurnError>> + Send;

    /// Embeds texts.
    fn embed(
        &self,
        request: &EmbedRequest,
    ) -> impl Future<Output = Result<EmbedReply, TurnError>> + Send;
}

/// Runs `request` on `model` and answers with the reply: the one path a chat, task or embedding
/// turn takes whatever the model is. A request of another kind is refused `Unsupported`.
pub async fn answer(
    model: &impl Model,
    request: &InferRequest,
    sink: &mut impl ChatSink,
) -> InferReply {
    match request {
        InferRequest::Chat(chat) => match model.chat(chat, sink).await {
            Ok(reply) => InferReply::Chat(reply),
            Err(error) => error.into(),
        },
        InferRequest::Task(task) => match model.task(task, sink).await {
            Ok(reply) => InferReply::Chat(reply),
            Err(error) => error.into(),
        },
        InferRequest::Embed(embed) => match model.embed(embed).await {
            Ok(reply) => InferReply::Embed(reply),
            Err(error) => error.into(),
        },
        _ => InferReply::Refused(InferRefusal::Unsupported),
    }
}
