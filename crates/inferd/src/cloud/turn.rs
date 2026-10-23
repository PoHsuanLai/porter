//! One chat turn to a hosted model: the caps are asked again, the key is fetched from accountd
//! for this turn only, the request goes out through stoker's `OpenAiCompat` driver over the TLS
//! transport, the events stream back, and what the reply says it used is metered.
//!
//! The key lives from `Peer.ResolveKey` to the end of the turn: it is read from a sealed
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
use crate::settings::SpendLine;
use model_openai_compat::OpenAiCodec;
use model_provider as sp;
use model_provider::{Provider, Retrying};
use model_wire::Driver;
use porter_core::{AppId, Tokens};
use porter_infer::{
    ChatReply, ChatRequest, ChatSink, EmbedReply, EmbedRequest, Flow, InferRefusal, Knob, Model,
    ModelCard, ModelError, ServedBy, SpendVerdict, TaskRequest, TokenUsage, TurnError,
};
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, PoisonError};

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
    // A longest reply of 0 is one nobody stated (a server of the person's own): the default.
    let limit = model
        .entry
        .capabilities
        .text_out
        .as_ref()
        .map(|text| text.max_output.0)
        .filter(|most| *most > 0)
        .unwrap_or(DEFAULT_MAX_OUTPUT)
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

/// Where a hosted turn's events go: to the sink, and into the reply being gathered.
struct Forward<'a, S> {
    sink: &'a mut S,
    gathered: bridge::Gathered,
}

impl<S: ChatSink> sp::TurnSink for Forward<'_, S> {
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        self.gathered.take(&event);
        match bridge::event(&event) {
            Some(out) => match self.sink.event(out) {
                Flow::Stop => sp::Flow::Stop,
                _ => sp::Flow::Continue,
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

/// The most a hosted turn may take once its connection is open, however many keep-alive frames
/// arrive: the per-stage waits bound a silent server, never one that trickles. Three minutes for
/// a first byte, then the longest reply inferd asks for (16 384 tokens) at a slow twenty a second
/// is about fourteen more; an overrun is a timeout, which keeps the turn's hold charged.
const TURN_DEADLINE: sp::WaitMs = sp::WaitMs(900_000);

/// A request on the hosted model a [`CloudPin`] names, as a [`Model`]. It exists only while the
/// caps allow the app another turn on the account, and holds the request's attachments.
#[derive(Debug)]
pub struct CloudModel<'a> {
    cloud: &'a Cloud,
    pin: &'a CloudPin,
    served: ServedBy,
    tier: porter_core::Tier,
    frames: Frames,
}

impl<'a> CloudModel<'a> {
    /// The request on `pin`'s model, with its `attachments`: refused `OverBudget` when the caps
    /// are asked again and say stop, failed `Unreadable` when an attachment cannot be read.
    pub fn open(
        cloud: &'a Cloud,
        pin: &'a CloudPin,
        served: &ServedBy,
        tier: porter_core::Tier,
        attachments: Vec<OwnedFd>,
    ) -> Result<Self, TurnError> {
        if cloud.spend(pin.line, &pin.app, &pin.model) == SpendVerdict::Stop {
            return Err(InferRefusal::OverBudget.into());
        }
        let frames = Frames::read(attachments).map_err(|_| ModelError::Unreadable)?;
        Ok(Self {
            cloud,
            pin,
            served: served.clone(),
            tier,
            frames,
        })
    }

    /// `turn` on the hosted model, its events going to `sink`: the key is fetched for this turn
    /// only, the worst cost is held before it goes out, and what it used is settled.
    async fn run(
        &self,
        mut turn: sp::TurnRequest,
        temperature: Temperature,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let (cloud, pin) = (self.cloud, self.pin);
        let Some(grant) = pin.model.grant() else {
            return Err(InferRefusal::NeedsGrant.into());
        };
        let key = cloud.key(grant).await.map_err(fault)?;
        let reach = &pin.model.reach;
        let shape = BodyShape::of(&reach.provider, &pin.model.entry.id.0, temperature);
        let Some(transport) = cloud.transport(&pin.model, &key, shape) else {
            return Err(ModelError::Unreachable.into());
        };
        drop(key);
        turn.limits.deadline = Some(TURN_DEADLINE);
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
            return Err(InferRefusal::OverBudget.into());
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
        let mut forward = Forward {
            sink,
            gathered: bridge::Gathered::for_turn(&turn),
        };
        match provider.turn(&turn, &mut forward).await {
            Ok(end) => {
                let (chat, why) = forward.gathered.chat_reply_noted(&end, self.served.clone());
                crate::runner::note_no_scores(why);
                Ok(chat)
            }
            Err(error) => Err(bridge::model_error(&error).into()),
        }
    }
}

impl Model for CloudModel<'_> {
    fn card(&self) -> &ModelCard {
        &self.pin.model.card
    }

    async fn chat(
        &self,
        request: &ChatRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let temperature = match request.control.sampling {
            Knob::Set(_) => Temperature::Sent,
            Knob::Off => Temperature::ProviderDefault,
        };
        let target = remote_target(&self.pin.model);
        let turn = bridge::chat_turn_for(&target, request, &self.frames)
            .map_err(|_| InferRefusal::Unsupported)?;
        self.run(turn, temperature, sink).await
    }

    async fn task(
        &self,
        request: &TaskRequest,
        sink: &mut impl ChatSink,
    ) -> Result<ChatReply, TurnError> {
        let target = remote_target(&self.pin.model);
        let turn = bridge::task_turn_for(&target, request, self.tier)
            .map_err(|_| InferRefusal::Unsupported)?;
        self.run(turn, Temperature::ProviderDefault, sink).await
    }

    /// A hosted model takes no embeddings.
    async fn embed(&self, _: &EmbedRequest) -> Result<EmbedReply, TurnError> {
        Err(InferRefusal::Unsupported.into())
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

/// Keeps what the provider reports an attempt cost when it ends ([`sp::TurnSink::ended`]), and
/// passes it on to the session's sink.
struct Watched<'a, K> {
    inner: &'a mut K,
    report: Option<sp::UsageReport>,
}

impl<K: sp::TurnSink> sp::TurnSink for Watched<'_, K> {
    fn event(&mut self, event: sp::TurnEvent) -> sp::Flow {
        self.inner.event(event)
    }

    fn ended(&mut self, report: sp::UsageReport) {
        self.report = Some(report);
        self.inner.ended(report);
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
            report: None,
        };
        // A guard dropped before this returns (the client went away and the task was aborted)
        // leaves its hold counted.
        let result = self.inner.turn(request, &mut watched).await;
        guard.settle(charge_of(watched.report));
        result
    }
}

/// What to charge an attempt, from the provider's report of how it ended: its count when the
/// server gave one, nothing when no model ran, and the hold (`None`) when the model may have run
/// and no count came, or no report did.
fn charge_of(report: Option<sp::UsageReport>) -> Option<TokenUsage> {
    match report {
        Some(sp::UsageReport::Known(used)) => Some(bridge::usage(used)),
        Some(sp::UsageReport::Nothing) => Some(TokenUsage {
            input: Tokens(0),
            output: Tokens(0),
            cached: Tokens(0),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A count is charged as given, a request no model ran is charged nothing, and an attempt
    /// that may have run without a count (or sent no report) keeps the hold.
    #[test]
    fn an_attempt_is_charged_from_how_the_provider_says_it_ended() {
        let used = sp::TurnUsage {
            input: sp::Tokens(10),
            output: sp::Tokens(5),
            ..sp::TurnUsage::default()
        };
        assert_eq!(
            charge_of(Some(sp::UsageReport::Known(used))),
            Some(bridge::usage(used))
        );
        let none = charge_of(Some(sp::UsageReport::Nothing)).expect("a charge");
        assert_eq!(
            (none.input, none.output, none.cached),
            (Tokens(0), Tokens(0), Tokens(0))
        );
        let seen = sp::UsageReport::Unknown {
            seen: sp::TurnUsage::default(),
        };
        assert_eq!(charge_of(Some(seen)), None);
        assert_eq!(charge_of(None), None);
    }
}
