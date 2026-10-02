//! The seam every wire adapter implements (Chat Completions, Responses, Messages,
//! generateContent, Ollama's API, ComfyUI workflows), and the fake tests drive.

use crate::error::ModelError;
use crate::event::{Flow, InferEvent};
use crate::reply::{ChatReply, EmbedReply};
use crate::request::{ChatRequest, EmbedRequest};
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

/// One model behind one account.
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
    ) -> impl Future<Output = Result<ChatReply, ModelError>> + Send;

    /// Embeds texts.
    fn embed(
        &self,
        request: &EmbedRequest,
    ) -> impl Future<Output = Result<EmbedReply, ModelError>> + Send;
}
