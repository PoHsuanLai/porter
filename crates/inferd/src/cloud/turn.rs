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
use super::spend::{SpendGuard, worst_case};
use super::wire::{BodyShape, Temperature, flavor_of};
use crate::bridge::{self, DefaultSampling, Frames, JsonReply, Target};
use crate::runner::{RETRY, TokioSleep};
use crate::serve::TurnStep;
use crate::settings::SpendLine;
use model_openai_compat::OpenAiCodec;
use model_provider as sp;
use model_provider::{Provider, Retrying};
use model_wire::Driver;
use porter_core::{AppId, Tokens};
use porter_infer::{
    InferRefusal, InferReply, InferRequest, Knob, ModelError, ServedBy, SpendVerdict, TokenUsage,
};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, PoisonError};
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
        json: model
            .entry
            .capabilities
            .text_out
            .as_ref()
            .map(|text| JsonReply::of(&text.structured))
            .unwrap_or_default(),
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
    // The worst this turn can cost is held before it goes out, so turns that run together see
    // each other's holds. The prompt's size is read off the request as it will be sent.
    let prompt_bytes = serde_json::to_vec(&turn).map_or(0, |body| body.len());
    let Some(first) = cloud.ledger().reserve(
        pin.line,
        &pin.app,
        &pin.model.card.account,
        &pin.model.price(),
        worst_case(prompt_bytes, Tokens(turn.limits.max_output.0)),
        cloud.now(),
    ) else {
        return refused(InferRefusal::OverBudget);
    };
    let provider = Retrying::new(
        Billed {
            inner: Driver::new(OpenAiCodec::new(flavor_of(&reach.provider)), transport),
            cloud,
            pin,
            worst: worst_case(prompt_bytes, Tokens(turn.limits.max_output.0)),
            first: Mutex::new(Some(first)),
        },
        RETRY,
        TokioSleep,
    );
    let mut sink = Forward {
        steps,
        gathered: bridge::Gathered::for_turn(&turn),
    };
    match provider.turn(&turn, &mut sink).await {
        Ok(end) => {
            let (chat, why) = sink.gathered.chat_reply_noted(&end, served.clone());
            crate::runner::note_no_scores(why);
            InferReply::Chat(chat)
        }
        Err(error) => InferReply::Failed(bridge::model_error(&error)),
    }
}

/// A provider whose every attempt is held against the caps before it goes out and settled when
/// it ends, so a retry that reached the provider is charged as the first try was.
struct Billed<'a, P> {
    inner: P,
    cloud: &'a Cloud,
    pin: &'a CloudPin,
    worst: TokenUsage,
    /// The hold taken for the first attempt, before it was known to be wanted.
    first: Mutex<Option<SpendGuard<'a>>>,
}

/// Remembers whether the session ended the turn early, since what an early end used is not known.
struct Watched<'a, K> {
    inner: &'a mut K,
    stopped: bool,
}

impl<K: sp::TurnSink> sp::TurnSink for Watched<'_, K> {
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        let flow = self.inner.event(event);
        self.stopped |= flow == sp::Flow::Stop;
        flow
    }
}

impl<P: Provider> Provider for Billed<'_, P> {
    async fn describe(&self) -> Result<Vec<sp::ModelInfo>, sp::ProviderError> {
        self.inner.describe().await
    }

    async fn turn<K: sp::TurnSink>(
        &self,
        request: &sp::TurnRequest,
        sink: &mut K,
    ) -> Result<sp::TurnEnd, sp::ProviderError> {
        let held = self
            .first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let guard = match held {
            Some(guard) => guard,
            None => self
                .cloud
                .ledger()
                .reserve(
                    self.pin.line,
                    &self.pin.app,
                    &self.pin.model.card.account,
                    &self.pin.model.price(),
                    self.worst,
                    self.cloud.now(),
                )
                .ok_or_else(|| sp::ProviderError::Refused("a spend cap is reached".to_owned()))?,
        };
        let mut watched = Watched {
            inner: sink,
            stopped: false,
        };
        // A guard dropped before this returns (the client went away and the task was aborted)
        // leaves its hold counted.
        let result = self.inner.turn(request, &mut watched).await;
        let used = match &result {
            Ok(end) if !watched.stopped && end.usage != sp::TurnUsage::default() => {
                Some(bridge::usage(end.usage))
            }
            Err(error) if refused_before_any_work(error) => Some(nothing_used()),
            _ => None,
        };
        guard.settle(used);
        result
    }
}

/// Whether the provider turned the request away before it generated anything, so it billed
/// nothing: it could not be reached, was not ready, was busy, refused the credentials or the
/// account, or refused the request as written. A timeout, a server error part-way or a reply
/// that could not be read may have come after work the provider bills, and keep the hold.
fn refused_before_any_work(error: &sp::ProviderError) -> bool {
    matches!(
        error,
        sp::ProviderError::Unreachable
            | sp::ProviderError::NotReady
            | sp::ProviderError::RateLimited(_)
            | sp::ProviderError::Unauthorized
            | sp::ProviderError::AuthRejected(_)
            | sp::ProviderError::PaymentRequired(_)
            | sp::ProviderError::ContextOverflow { .. }
            | sp::ProviderError::BadRequest(_)
            | sp::ProviderError::Refused(_)
    )
}

/// The usage of a turn the provider turned away.
fn nothing_used() -> TokenUsage {
    TokenUsage {
        input: Tokens(0),
        output: Tokens(0),
        cached: Tokens(0),
    }
}
