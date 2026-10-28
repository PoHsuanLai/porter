//! A model that echoes the last user text, as a stream of one text delta and a usage event.

use crate::{
    ChatReply, ChatRequest, ChatSink, EmbedReply, EmbedRequest, EmbedVector, InferEvent,
    MessagePart, Model, ModelCard, ServedBy, StopReason, TaskRequest, TokenUsage, TurnError,
};
use porter_capability::Capability;
use porter_capability::ModelId;
use porter_capability::{Billing, Locality};
use porter_core::AccountId;
use porter_ids::Tokens;

/// An on-device model of the fake runtime.
#[derive(Debug, Clone)]
pub struct FakeModel {
    card: ModelCard,
}

impl FakeModel {
    /// The model `model` of `account`, on this computer, free, with `capabilities`.
    pub fn on_device(account: AccountId, model: ModelId, capabilities: Vec<Capability>) -> Self {
        Self {
            card: ModelCard {
                account,
                model,
                locality: Locality::OnDevice,
                billing: Billing::Free,
                capabilities,
            },
        }
    }

    fn served(&self) -> ServedBy {
        ServedBy {
            account: self.card.account.clone(),
            model: self.card.model.clone(),
            locality: self.card.locality.clone(),
        }
    }
}

impl Model for FakeModel {
    fn card(&self) -> &ModelCard {
        &self.card
    }

    async fn chat(
        &self,
        request: &ChatRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let text = request
            .messages
            .iter()
            .flat_map(|m| &m.parts)
            .filter_map(|part| match part {
                MessagePart::Text(text) => Some(text.as_str()),
                MessagePart::Image(_)
                | MessagePart::ToolCall(_)
                | MessagePart::ToolResult(_)
                | MessagePart::Thought(_) => None,
            })
            .next_back()
            .unwrap_or_default()
            .to_owned();
        let usage = TokenUsage {
            input: Tokens(1),
            output: Tokens(1),
            cached: Tokens(0),
            charge: None,
        };
        let _ = sink.event(InferEvent::TextDelta(text.clone()));
        let _ = sink.event(InferEvent::Usage(usage));
        Ok(ChatReply::new(
            text,
            StopReason::EndTurn,
            usage,
            self.served(),
        ))
    }

    async fn task(
        &self,
        request: &TaskRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let text = request.input.clone();
        let usage = TokenUsage {
            input: Tokens(1),
            output: Tokens(1),
            cached: Tokens(0),
            charge: None,
        };
        let _ = sink.event(InferEvent::TextDelta(text.clone()));
        let _ = sink.event(InferEvent::Usage(usage));
        Ok(ChatReply::new(
            text,
            StopReason::EndTurn,
            usage,
            self.served(),
        ))
    }

    async fn embed(&self, request: &EmbedRequest) -> Result<EmbedReply, TurnError> {
        let vectors = request
            .inputs
            .iter()
            .map(|input| EmbedVector(vec![input.len() as f32]))
            .collect();
        let usage = TokenUsage {
            input: Tokens(1),
            output: Tokens(0),
            cached: Tokens(0),
            charge: None,
        };
        Ok(EmbedReply::new(vectors, usage, self.served()))
    }
}
