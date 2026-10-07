//! One chat turn to a hosted model: the caps are asked again, the key is fetched from accountd
//! for this turn only, the request goes out through stoker's `OpenAiCompat` driver over the TLS
//! transport, the events stream back, and what the reply says it used is metered.
//!
//! The key lives from `Peer.ResolveKey` to the end of this function: it is read from a sealed
//! descriptor into a [`porter_core::SecretText`], copied once into the endpoint's bearer header of
//! a transport built for this turn, and dropped with them. It is never put in a log line, an
//! event, an audit entry or a file; an error from the provider is classified without its body.

use super::Cloud;
use super::accountd::AccountdFault;
use super::models::RemoteModel;
use super::transport::ShapedTransport;
use super::wire::{BodyShape, Temperature, flavor_of};
use crate::bridge::{self, DefaultSampling, Frames, Target};
use crate::runner::{RETRY, TokioSleep};
use crate::serve::TurnStep;
use crate::settings::SpendLine;
use model_openai_compat::OpenAiCodec;
use model_provider as sp;
use model_provider::{Provider, Retrying};
use model_wire::Driver;
use porter_core::AppId;
use porter_infer::{
    InferRefusal, InferReply, InferRequest, Knob, ModelError, ServedBy, SpendVerdict, cost,
};
use std::os::fd::OwnedFd;
use std::sync::Arc;
use tokio::sync::mpsc;

/// The longest reply asked for when the app names no limit: a hosted model's entry says what it
/// can write (a hundred and twenty-eight thousand tokens), and some gateways hold back credit for
/// the whole of what is asked.
const DEFAULT_MAX_OUTPUT: u32 = 16_384;

/// What the router pinned a session to, when that is a hosted model.
#[derive(Debug, Clone)]
pub struct CloudPin {
    /// The model and the reach.
    pub model: Arc<RemoteModel>,
    /// The app that opened the session.
    pub app: AppId,
    /// The spend rows in force when the session was routed.
    pub line: SpendLine,
}

/// The turn target of a hosted model on the reach chosen: the provider's own name for it, the
/// provider's defaults for sampling, a bounded reply.
pub fn remote_target(model: &RemoteModel) -> Target {
    let limit = model
        .entry
        .capabilities
        .text_out
        .as_ref()
        .map_or(DEFAULT_MAX_OUTPUT, |text| text.max_output.0)
        .min(DEFAULT_MAX_OUTPUT);
    Target {
        name: sp::ModelName(model.reach.model.0.clone()),
        sampling: DefaultSampling::Provider,
        max_output: sp::Tokens(limit),
        flavor: Some(flavor_of(&model.reach.provider)),
    }
}

/// Where a hosted turn's events go: to the session, and into the reply being gathered.
struct Forward {
    steps: mpsc::UnboundedSender<TurnStep>,
    gathered: bridge::Gathered,
}

impl sp::TurnSink for Forward {
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        self.gathered.take(&event);
        match bridge::event(&event) {
            Some(out) => match self.steps.send(TurnStep::Event(out)) {
                Ok(()) => sp::Flow::Continue,
                Err(_) => sp::Flow::Stop,
            },
            None => sp::Flow::Continue,
        }
    }
}

fn fault(fault: AccountdFault) -> ModelError {
    match fault {
        AccountdFault::Refused => ModelError::Unauthorized,
        AccountdFault::Unreachable | AccountdFault::Unreadable => ModelError::Unreachable,
    }
}

/// The turn's request, and how its body is shaped for the provider; `None` for a request a hosted
/// model does not take (embeddings, speech, computer use).
fn turn_of(
    pin: &CloudPin,
    request: &InferRequest,
    frames: &Frames,
    tier: porter_core::Tier,
) -> Option<Result<(sp::TurnRequest, Temperature), bridge::BridgeError>> {
    let target = remote_target(&pin.model);
    match request {
        InferRequest::Chat(chat) => {
            let temperature = match chat.control.sampling {
                Knob::Set(_) => Temperature::Sent,
                Knob::Off => Temperature::ProviderDefault,
            };
            Some(bridge::chat_turn_for(&target, chat, frames).map(|turn| (turn, temperature)))
        }
        InferRequest::Task(task) => Some(
            bridge::task_turn_for(&target, task, tier)
                .map(|turn| (turn, Temperature::ProviderDefault)),
        ),
        _ => None,
    }
}

/// Runs `request` on the hosted model `pin` names, streaming its events to `steps`, and answers
/// with the reply.
pub async fn reply(
    cloud: &Cloud,
    pin: &CloudPin,
    served: &ServedBy,
    tier: porter_core::Tier,
    steps: mpsc::UnboundedSender<TurnStep>,
    request: &InferRequest,
    attachments: Vec<OwnedFd>,
) -> InferReply {
    let refused = |why| InferReply::Refused(why);
    if cloud.spend(pin.line, &pin.app, &pin.model) == SpendVerdict::Stop {
        return refused(InferRefusal::OverBudget);
    }
    let Ok(frames) = Frames::read(attachments) else {
        return InferReply::Failed(ModelError::Unreadable);
    };
    let (turn, temperature) = match turn_of(pin, request, &frames, tier) {
        Some(Ok(built)) => built,
        Some(Err(_)) | None => return refused(InferRefusal::Unsupported),
    };
    let Some(grant) = pin.model.grant() else {
        return refused(InferRefusal::NeedsGrant);
    };
    let key = match cloud.key(grant).await {
        Ok(key) => key,
        Err(why) => return InferReply::Failed(fault(why)),
    };
    let reach = &pin.model.reach;
    let shape = BodyShape::of(&reach.provider, &pin.model.entry.id.0, temperature);
    let Some(transport) = cloud.transport(&pin.model, &key, shape) else {
        return InferReply::Failed(ModelError::Unreachable);
    };
    drop(key);
    let provider = provider_over(transport, &reach.provider);
    let mut sink = Forward {
        steps,
        gathered: bridge::Gathered::default(),
    };
    match provider.turn(&turn, &mut sink).await {
        Ok(end) => {
            let chat = sink.gathered.chat_reply(&end, served.clone());
            cloud.ledger().record(
                &pin.app,
                &pin.model.card.account,
                chat.usage,
                cost(chat.usage, &pin.model.price()),
                cloud.now(),
            );
            InferReply::Chat(chat)
        }
        Err(error) => InferReply::Failed(bridge::model_error(&error)),
    }
}

/// The provider a transport is driven through: stoker's chat-completions codec in the provider's
/// dialect, retried as inferd retries a local engine.
fn provider_over(
    transport: ShapedTransport,
    provider: &model_catalog::ProviderId,
) -> Retrying<Driver<OpenAiCodec, ShapedTransport>, TokioSleep> {
    Retrying::new(
        Driver::new(OpenAiCodec::new(flavor_of(provider)), transport),
        RETRY,
        TokioSleep,
    )
}
