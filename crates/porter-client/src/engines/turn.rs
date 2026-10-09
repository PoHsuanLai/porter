//! One request of a session, run on its engine: the request as stoker's turn (porter-bridge, the
//! mapping inferd uses), the key for this turn only, the events streamed to the session as they
//! come, the reply at the end.

use super::engine::{Engine, KeyUse};
use super::keys::KeySource;
use super::wire::{Temperature, provider};
use model_provider as sp;
use model_provider::Provider;
use porter_bridge::{DefaultSampling, Frames, Target, chat_turn_for, task_turn_for};
use porter_core::Tier;
use porter_infer::{
    InferEvent, InferRefusal, InferReply, InferRequest, Knob, ModelError, ServedBy,
};
use tokio::sync::mpsc::UnboundedSender;

/// What a session is pinned to when it opens: the engine its route chose.
#[derive(Debug, Clone)]
pub(crate) struct Pinned {
    pub(crate) engine: Engine,
    pub(crate) served: ServedBy,
    pub(crate) tier: Tier,
}

impl Pinned {
    fn target(&self) -> Target {
        Target {
            name: sp::ModelName(self.engine.served_name.clone()),
            sampling: DefaultSampling::Provider,
            max_output: sp::Tokens(self.engine.max_output.0),
            flavor: Some(self.engine.dialect.flavor()),
        }
    }
}

/// Where a turn's events go: to the session, and into the reply being gathered.
struct Forward<'a> {
    events: &'a UnboundedSender<InferEvent>,
    gathered: porter_bridge::Gathered,
}

impl sp::TurnSink for Forward<'_> {
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        self.gathered.take(&event);
        match porter_bridge::event(&event) {
            Some(out) => match self.events.send(out) {
                Ok(()) => sp::Flow::Continue,
                Err(_) => sp::Flow::Stop,
            },
            None => sp::Flow::Continue,
        }
    }
}

/// The turn for a request this host serves, and whether the app chose a sampling; `None` for a
/// kind of request it does not (embeddings, speech, computer use).
fn turn_of(
    pinned: &Pinned,
    request: &InferRequest,
    frames: &Frames,
) -> Option<Result<(sp::TurnRequest, Temperature), porter_bridge::BridgeError>> {
    let target = pinned.target();
    match request {
        InferRequest::Chat(chat) => {
            let temperature = match chat.control.sampling {
                Knob::Set(_) => Temperature::Sent,
                Knob::Off => Temperature::EngineDefault,
            };
            Some(chat_turn_for(&target, chat, frames).map(|turn| (turn, temperature)))
        }
        InferRequest::Task(task) => Some(
            task_turn_for(&target, task, pinned.tier)
                .map(|turn| (turn, Temperature::EngineDefault)),
        ),
        _ => None,
    }
}

/// Runs `request` on the engine `pinned` names, streaming its events to `events`, and answers
/// with the reply.
pub(crate) async fn reply(
    pinned: &Pinned,
    keys: &impl KeySource,
    request: &InferRequest,
    frames: &Frames,
    events: &UnboundedSender<InferEvent>,
) -> InferReply {
    let (turn, temperature) = match turn_of(pinned, request, frames) {
        Some(Ok(built)) => built,
        Some(Err(porter_bridge::BridgeError::Attachment)) => {
            return InferReply::Failed(ModelError::Unreadable);
        }
        Some(Err(_)) | None => return InferReply::Refused(InferRefusal::Unsupported),
    };
    let key = match pinned.engine.key {
        KeyUse::None => None,
        KeyUse::Bearer => match keys.key(&pinned.engine.account).await {
            Some(key) => Some(key),
            None => return InferReply::Refused(InferRefusal::NeedsGrant),
        },
    };
    let provider = provider(&pinned.engine, key.as_ref(), temperature);
    drop(key);
    let mut sink = Forward {
        events,
        gathered: porter_bridge::Gathered::for_turn(&turn),
    };
    match provider.turn(&turn, &mut sink).await {
        Ok(end) => InferReply::Chat(sink.gathered.chat_reply(&end, pinned.served.clone())),
        Err(error) => InferReply::Failed(porter_bridge::model_error(&error)),
    }
}
