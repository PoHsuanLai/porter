//! A model that echoes the last user text, as a stream of one text delta and a usage event.

use porter_core::{AccountId, Billing, Capability, Locality, ModelId, Tokens};
use porter_infer::{
    ChatReply, ChatRequest, ChatSink, EmbedReply, EmbedRequest, EmbedVector, InferEvent,
    MessagePart, Model, ModelCard, ModelError, ServedBy, StopReason, TokenUsage,
};

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
    ) -> Result<ChatReply, ModelError> {
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
        };
        let _ = sink.event(InferEvent::TextDelta(text.clone()));
        let _ = sink.event(InferEvent::Usage(usage));
        Ok(ChatReply {
            text,
            tool_calls: Vec::new(),
            stop: StopReason::EndTurn,
            thought: None,
            scores: None,
            usage,
            served: self.served(),
        })
    }

    async fn embed(&self, request: &EmbedRequest) -> Result<EmbedReply, ModelError> {
        let vectors = request
            .inputs
            .iter()
            .map(|input| EmbedVector(vec![input.len() as f32]))
            .collect();
        let usage = TokenUsage {
            input: Tokens(1),
            output: Tokens(0),
            cached: Tokens(0),
        };
        Ok(EmbedReply {
            vectors,
            usage,
            served: self.served(),
        })
    }
}
