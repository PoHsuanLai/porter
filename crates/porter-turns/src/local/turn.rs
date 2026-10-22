//! A request on a model of this computer, as a [`Model`]: chat (validated and repaired when the
//! request's shape asks), a task, embeddings. Its events go to the sink it is given.

use super::{EngineUse, chat_provider, client, note_no_scores, run_turn};
use crate::bridge::{self, Frames};
use crate::structured::{self, Checked, Limits, Shaping};
use model_openai_compat::OpenAiCodec;
use model_provider as sp;
use model_provider::Embedder;
use porter_core::Tier;
use porter_infer::{
    ChatReply, ChatRequest, ChatSink, EmbedReply, EmbedRequest, Flow, InferEvent, InferRefusal,
    Model, ModelCard, ServedBy, StopReason, TaskRequest, TokenUsage, TurnError,
};
use porter_router::local::LocalModel;

/// One request on `model`, which tells `engines` it is in use while the request runs.
#[derive(Debug)]
pub struct LocalTurn<'a, U> {
    /// The model, on the engine instance the request was seated on.
    pub model: &'a LocalModel,
    /// What the reply says answered.
    pub served: &'a ServedBy,
    /// The owner of the engines.
    pub engines: &'a U,
    /// The session's tier, which a task is worded by.
    pub tier: Tier,
    /// What a structured reply may leave open and how often it is repaired.
    pub limits: Limits,
    /// The bytes of the request's attachments.
    pub frames: &'a Frames,
    /// The most a reply may write, whatever the request asks (the model's most, the person's
    /// length and the window the engine runs with: `LocalModel::reply_ceiling`).
    pub ceiling: sp::Tokens,
}

fn flow(sent: Flow) -> sp::Flow {
    match sent {
        Flow::Stop => sp::Flow::Stop,
        _ => sp::Flow::Continue,
    }
}

impl<U: EngineUse> LocalTurn<'_, U> {
    /// `turn`, asking for no longer a reply than the ceiling: a request that asks for more gets
    /// the most the model may write, and one that leaves it open gets the model's served reply
    /// unless the ceiling is lower.
    fn within_ceiling(&self, mut turn: sp::TurnRequest) -> sp::TurnRequest {
        turn.limits.max_output = turn.limits.max_output.min(self.ceiling);
        turn
    }

    /// `turn` on the model's engine, `shaping` saying whether its reply is validated and repaired.
    async fn run(
        &self,
        turn: &sp::TurnRequest,
        shaping: Shaping,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let Some(provider) = chat_provider(self.model) else {
            return Err(InferRefusal::Unsupported.into());
        };
        if let Shaping::Checked(checked) = shaping {
            return self.checked(&provider, *checked, turn, sink).await;
        }
        let (reply, why) = run_turn(
            &provider,
            turn,
            self.served,
            |event| flow(sink.event(event)),
            || self.engines.used(&self.model.spec.id),
        )
        .await;
        note_no_scores(why);
        Ok(reply?)
    }

    /// A turn whose reply is validated, and repaired once, before the app is told it.
    async fn checked(
        &self,
        provider: &impl sp::Provider,
        checked: Checked,
        turn: &sp::TurnRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let mut forward = |event| flow(sink.event(event));
        let touch = || self.engines.used(&self.model.spec.id);
        let valid = structured::run(provider, checked, turn, &mut forward, touch).await?;
        let _ = sink.event(InferEvent::TextDelta(valid.text.clone()));
        let scores = match bridge::turn_scores(turn, valid.first_token.as_ref()) {
            Some(Ok(scores)) => Some(scores),
            Some(Err(why)) => {
                note_no_scores(Some(why));
                None
            }
            None => None,
        };
        let mut reply = ChatReply::new(
            valid.text,
            StopReason::EndTurn,
            valid.usage,
            self.served.clone(),
        );
        if let Some(thought) = valid.thought {
            reply = reply.with_thought(thought);
        }
        if let Some(scores) = scores {
            reply = reply.with_scores(scores);
        }
        Ok(reply)
    }
}

impl<U: EngineUse> Model for LocalTurn<'_, U> {
    fn card(&self) -> &ModelCard {
        &self.model.card
    }

    async fn chat(
        &self,
        request: &ChatRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let turn = bridge::chat_turn(self.model, request, self.frames)
            .map(|turn| self.within_ceiling(turn))
            .map_err(|_| InferRefusal::Unsupported)?;
        let shaping = structured::shaping(self.model, request, self.limits);
        self.run(&turn, shaping, sink).await
    }

    async fn task(
        &self,
        request: &TaskRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let turn = bridge::task_turn(self.model, request, self.tier)
            .map(|turn| self.within_ceiling(turn))
            .map_err(|_| InferRefusal::Unsupported)?;
        self.run(&turn, Shaping::Unchecked, sink).await
    }

    /// Embeddings cut into the batches the model's entry allows.
    async fn embed(&self, request: &EmbedRequest) -> Result<EmbedReply, TurnError> {
        let model = self.model;
        let Some(flavor) = model.flavor else {
            return Err(InferRefusal::Unsupported.into());
        };
        let Ok(turns) = bridge::embed_turns(model, request) else {
            return Err(InferRefusal::Unsupported.into());
        };
        let provider = OpenAiCodec::new(flavor).provider(client(model));
        let mut vectors = Vec::new();
        let mut input = sp::Tokens(0);
        for turn in &turns {
            let end = provider
                .embed(turn)
                .await
                .map_err(|error| bridge::model_error(&error))?;
            input = sp::Tokens(input.0.saturating_add(end.usage.input.0));
            vectors.extend(bridge::vectors(end.vectors));
            self.engines.used(&model.spec.id);
        }
        Ok(EmbedReply::new(
            vectors,
            TokenUsage {
                input: porter_core::Tokens(input.0),
                output: porter_core::Tokens(0),
                cached: porter_core::Tokens(0),
            },
            self.served.clone(),
        ))
    }
}
