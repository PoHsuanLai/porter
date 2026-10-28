//! One chat turn to a hosted model: the caps are asked again, the key is fetched from accountd
//! for this turn only, the request goes out through stoker's `OpenAiCompat` driver over the TLS
//! transport, the events stream back, and what the reply says it used and was charged is metered.
//!
//! The key lives from `Peer.ResolveKey` to the end of the turn: it is read from a sealed
//! descriptor into a [`porter_core::SecretText`], copied once into the endpoint's bearer header of
//! a transport built for this turn, and dropped with them. It is never put in a log line, an
//! event, an audit entry or a file; an error from the provider is classified without its body.

use super::Cloud;
use super::accountd::AccountdFault;
use super::models::RemoteModel;
use super::wire::{BodyShape, Temperature, flavor_of};
use crate::settings::SpendLine;
use model_openai_compat::OpenAiCodec;
use model_provider as sp;
use model_provider::{Provider, Retrying};
use model_wire::Driver;
use porter_core::AppId;
use porter_infer::{
    ChatReply, ChatRequest, ChatSink, EmbedReply, EmbedRequest, Flow, InferRefusal, Knob, Model,
    ModelCard, ModelError, ServedBy, SpendVerdict, TaskRequest, TokenUsage, TurnError,
};
use porter_turns::bridge::{self, DefaultSampling, Frames, JsonReply, Target};
use porter_turns::local::{RETRY, TokioSleep};
use std::os::fd::OwnedFd;
use std::sync::Arc;

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
/// is about fourteen more; an overrun is a timeout.
const TURN_DEADLINE: sp::WaitMs = sp::WaitMs(900_000);

/// A request on the hosted model a [`CloudPin`] names, as a [`Model`]. It exists only while the
/// caps allow the app another turn on the account, and holds the request's attachments.
#[derive(Debug)]
pub struct CloudModel<'a> {
    cloud: &'a Cloud,
    pin: &'a CloudPin,
    served: ServedBy,
    tier: porter_capability::Tier,
    frames: Frames,
}

impl<'a> CloudModel<'a> {
    /// The request on `pin`'s model, with its `attachments`: refused `OverBudget` when the caps
    /// are asked again and say stop, failed `Unreadable` when an attachment cannot be read.
    pub fn open(
        cloud: &'a Cloud,
        pin: &'a CloudPin,
        served: &ServedBy,
        tier: porter_capability::Tier,
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
    /// only, and what each attempt used and was charged is counted when it ends.
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
        let provider = Retrying::new(
            Billed {
                inner: Driver::new(OpenAiCodec::new(flavor_of(&reach.provider)), transport),
                cloud,
                pin,
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
                porter_turns::local::note_no_scores(why);
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

/// A provider whose every attempt is asked of the caps before it goes out and counted when it
/// ends, so a retry that reached the provider is counted as the first try was.
struct Billed<'a, P> {
    inner: P,
    cloud: &'a Cloud,
    pin: &'a CloudPin,
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

    fn charges(&self) -> sp::Charges {
        self.inner.charges()
    }

    async fn turn<K: sp::TurnSink>(
        &self,
        request: &sp::TurnRequest,
        sink: &mut K,
    ) -> Result<sp::TurnEnd, sp::ProviderError> {
        // The caps compare what providers have reported so far, so a retry can be stopped by the
        // attempt before it; an attempt already running is never cut off.
        let (app, line) = (&self.pin.app, self.pin.line);
        if self.cloud.spend(line, app, &self.pin.model) == SpendVerdict::Stop {
            return Err(sp::ProviderError::Refused(
                "a spend cap is reached".to_owned(),
            ));
        }
        let mut watched = Watched {
            inner: sink,
            report: None,
        };
        let result = self.inner.turn(request, &mut watched).await;
        if let Some(used) = counted(watched.report) {
            let account = &self.pin.model.card.account;
            self.cloud
                .ledger()
                .record(app, account, used, self.cloud.now());
        }
        result
    }
}

/// What to count for an attempt, from the provider's report of how it ended: its usage and charge
/// when the server gave a count, nothing when no model ran, and what was seen (an unpriced turn,
/// unless the provider had already said the charge) when the model may have run and no count
/// came. No charge is ever estimated.
fn counted(report: Option<sp::UsageReport>) -> Option<TokenUsage> {
    match report {
        Some(sp::UsageReport::Known(used)) => Some(bridge::usage(used)),
        Some(sp::UsageReport::Unknown { seen }) => Some(bridge::usage(seen)),
        Some(sp::UsageReport::Nothing) => None,
        // No report at all, or one a later stoker adds: a model may have run.
        _ => Some(bridge::usage(sp::TurnUsage::default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_ids::Tokens;
    use rust_decimal::Decimal;

    /// A count is counted as given with its charge, a request no model ran is counted as
    /// nothing, and an attempt that may have run without a count (or sent no report) is counted
    /// as a turn with what was seen and no charge.
    #[test]
    fn an_attempt_is_counted_from_how_the_provider_says_it_ended() {
        let charge = sp::Money::usd(Decimal::new(95, 5));
        let used = sp::TurnUsage {
            input: sp::Tokens(10),
            output: sp::Tokens(5),
            charge: Some(charge),
            ..sp::TurnUsage::default()
        };
        let known = counted(Some(sp::UsageReport::Known(used))).expect("counted");
        assert_eq!(known, bridge::usage(used));
        assert_eq!(known.charge, Some(charge));
        assert_eq!(counted(Some(sp::UsageReport::Nothing)), None);
        let seen = sp::TurnUsage {
            input: sp::Tokens(3),
            ..sp::TurnUsage::default()
        };
        let cut = counted(Some(sp::UsageReport::Unknown { seen })).expect("counted");
        assert_eq!((cut.input, cut.charge), (Tokens(3), None));
        let silent = counted(None).expect("counted");
        assert_eq!(
            (silent.input, silent.output, silent.charge),
            (Tokens(0), Tokens(0), None)
        );
    }
}
