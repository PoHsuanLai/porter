//! The wire adapters as code: one variant per built adapter (`porter_core::capability::LlmWire`
//! plus ComfyUI workflows). None is built yet, so the set is empty and uninhabited.

use porter_infer::{
    ChatReply, ChatRequest, ChatSink, EmbedReply, EmbedRequest, Model, ModelCard, ModelError,
};

/// Every built adapter's model.
#[derive(Debug)]
pub enum AdapterModel {}

impl Model for AdapterModel {
    fn card(&self) -> &ModelCard {
        match *self {}
    }

    async fn chat(&self, _: &ChatRequest, _: &mut impl ChatSink) -> Result<ChatReply, ModelError> {
        match *self {}
    }

    async fn embed(&self, _: &EmbedRequest) -> Result<EmbedReply, ModelError> {
        match *self {}
    }
}
