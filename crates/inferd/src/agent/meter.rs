//! What a request cost and who is told: the tokens and the charge read from the provider's own
//! usage figures (a charge only from a provider that reports one), counted against the account's
//! and the program's caps, and one audit line. The line holds the program (as its app), the
//! route, the tokens, the charge when the provider reported one and the class; never a prompt, a
//! reply, a key or the token.

use super::fail::Shape;
use super::session::Ctx;
use crate::cloud::Cloud;
use model_http::SseDecoder;
use model_provider::{Charges, Money};
use porter_capability::Locality;
use porter_ids::{Bytes, Count, Tokens};
use porter_infer::{AuditEntry, ServedBy, TokenUsage, Why};
use porter_turns::audit::AuditOut;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

/// A provider's usage figures as they arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Seen {
    input: Option<u32>,
    output: Option<u32>,
    cached: Option<u32>,
    charge: Option<Money>,
}

impl Seen {
    /// The figures, when the provider gave any.
    pub fn usage(self) -> Option<TokenUsage> {
        let given = self.input.is_some() || self.output.is_some() || self.charge.is_some();
        given.then(|| TokenUsage {
            input: Tokens(self.input.unwrap_or(0)),
            output: Tokens(self.output.unwrap_or(0)),
            cached: Tokens(self.cached.unwrap_or(0)),
            charge: self.charge,
        })
    }

    /// Takes the `usage` object of an Anthropic message, message_start or message_delta. Input
    /// counts the cached and the cache-written tokens too: they are billed as input.
    fn anthropic(&mut self, usage: &Value) {
        let number = |name: &str| usage.get(name).and_then(Value::as_u64);
        let to_u32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        let read = number("cache_read_input_tokens");
        let written = number("cache_creation_input_tokens");
        // A later event repeating `input_tokens` without the cache figures must not undo the total.
        let complete = self.input.is_none() || read.is_some() || written.is_some();
        if let Some(input) = number("input_tokens").filter(|_| complete) {
            let total = input + read.unwrap_or(0) + written.unwrap_or(0);
            self.input = Some(to_u32(total));
            self.cached = Some(to_u32(read.unwrap_or(0)));
        }
        if let Some(output) = number("output_tokens") {
            self.output = Some(to_u32(output));
        }
    }

    /// Takes the `usage` object of an OpenAI completion or final chunk.
    fn openai(&mut self, usage: &Value) {
        let to_u32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        if let Some(input) = usage.get("prompt_tokens").and_then(Value::as_u64) {
            self.input = Some(to_u32(input));
        }
        if let Some(output) = usage.get("completion_tokens").and_then(Value::as_u64) {
            self.output = Some(to_u32(output));
        }
        if let Some(cached) = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
        {
            self.cached = Some(to_u32(cached));
        }
    }

    fn take(&mut self, shape: Shape, value: &Value) {
        match shape {
            Shape::Anthropic => {
                if let Some(usage) = value.get("usage") {
                    self.anthropic(usage);
                }
                if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                    self.anthropic(usage);
                }
            }
            Shape::OpenAi => {
                if let Some(usage) = value.get("usage").filter(|u| u.is_object()) {
                    self.openai(usage);
                }
            }
        }
    }
}

/// Reads usage out of a provider's reply as its bytes arrive.
#[derive(Debug)]
pub struct Reading {
    shape: Shape,
    /// Whether the provider says what each reply cost; only then is a charge looked for.
    charges: Charges,
    seen: Seen,
    events: Option<SseDecoder>,
}

impl Reading {
    /// A reader of a stream of server-sent events from a provider that reports `charges`.
    pub fn of_stream(shape: Shape, charges: Charges) -> Self {
        Self {
            shape,
            charges,
            seen: Seen::default(),
            events: Some(SseDecoder::new()),
        }
    }

    /// A reader of one JSON body, which `finish_body` is given whole.
    pub fn of_body(shape: Shape, charges: Charges) -> Self {
        Self {
            shape,
            charges,
            seen: Seen::default(),
            events: None,
        }
    }

    /// Feeds a piece of a stream.
    pub fn feed(&mut self, bytes: &[u8]) {
        let Some(decoder) = self.events.as_mut() else {
            return;
        };
        let Ok(events) = decoder.feed(bytes) else {
            return;
        };
        for event in events {
            self.take(&event.data);
        }
    }

    /// Reads the whole body of a non-streaming reply.
    pub fn finish_body(&mut self, body: &[u8]) {
        if let Ok(text) = std::str::from_utf8(body) {
            self.take(text);
        }
    }

    /// Takes one JSON document (an event's data, or a whole body).
    fn take(&mut self, text: &str) {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            self.seen.take(self.shape, &value);
        }
        let wants_charge = self.shape == Shape::OpenAi && self.charges == Charges::PerReply;
        if let Some(charge) = reported_charge(text).filter(|_| wants_charge) {
            self.seen.charge = Some(charge);
        }
    }

    /// What was seen.
    pub fn seen(&self) -> Seen {
        self.seen
    }
}

#[derive(Deserialize)]
struct Frame<'a> {
    #[serde(borrow)]
    usage: Option<UsageText<'a>>,
}

#[derive(Deserialize)]
struct UsageText<'a> {
    #[serde(borrow)]
    cost: Option<&'a RawValue>,
}

/// The charge in an OpenAI-shaped document that carries one, `usage.cost` as OpenRouter writes it
/// (credits, one credit a US dollar), read from the number's own text so no float rounds it. The
/// same reading stoker's codec gives a turn it runs itself. Absent, not a number, negative or too
/// large is no charge: a figure that cannot be read is a missing one, never a failed request.
fn reported_charge(text: &str) -> Option<Money> {
    let frame: Frame<'_> = serde_json::from_str(text).ok()?;
    let digits = frame.usage?.cost?.get();
    let amount = Decimal::from_str_exact(digits)
        .or_else(|_| Decimal::from_scientific(digits))
        .ok()?;
    (!amount.is_sign_negative()).then(|| Money::usd(amount))
}

/// How a request to the provider ended, as far as its billing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The provider answered with success and the reply came whole.
    Answered,
    /// No model ran: the provider answered with an error, or the request never left (no
    /// connection, no handshake).
    NotRun,
    /// The model may have run and the reply did not come whole: cut off, timed out, or the
    /// agent went away.
    Unknown,
}

/// What to count for a request: what the provider reported when the reply came whole with figures
/// or came part way (a turn without a charge is unpriced, never worked out from a price), a turn
/// of no tokens when the model may have run and no figure came, and nothing (`None`) when no model
/// ran.
pub fn counted(end: End, seen: Seen) -> Option<TokenUsage> {
    match end {
        End::Answered | End::Unknown => Some(seen.usage().unwrap_or(NO_TOKENS)),
        End::NotRun => None,
    }
}

const NO_TOKENS: TokenUsage = TokenUsage {
    input: Tokens(0),
    output: Tokens(0),
    cached: Tokens(0),
    charge: None,
};

/// One audit line for a request.
#[derive(Debug)]
pub struct Line<'a> {
    /// The session.
    pub ctx: &'a Ctx,
    /// The account, the model and where it ran.
    pub served: ServedBy,
    /// The tokens, and the charge when the provider reported one.
    pub usage: TokenUsage,
    /// Bytes that left this computer (the request body, for a cloud route).
    pub sent: u64,
}

impl Line<'_> {
    /// Writes the line.
    pub fn write(self) {
        let out: &dyn AuditOut = &*self.ctx.audit;
        out.append(&AuditEntry {
            at: self.ctx.clock.now(),
            app: self.ctx.app.clone(),
            account: self.served.account,
            class: self.ctx.class,
            model: self.served.model,
            locality: self.served.locality.clone(),
            usage: self.usage,
            bytes_out: match self.served.locality {
                Locality::Cloud { .. } => Bytes(self.sent),
                _ => Bytes(0),
            },
            images: Count(0),
            audio_ms: Count(0),
            why: Some(Why::Named),
        });
    }
}

/// Counts a request in the ledger (`counted`, from [`counted`]: nothing when no model ran) against
/// the program and the account, and writes its audit line with what was counted.
pub fn settle(ctx: &Ctx, cloud: &Cloud, served: ServedBy, counted: Option<TokenUsage>, sent: u64) {
    if let Some(usage) = counted {
        cloud
            .ledger()
            .record(&ctx.app, &served.account, usage, cloud.now());
    }
    Line {
        ctx,
        served,
        usage: counted.unwrap_or(NO_TOKENS),
        sent,
    }
    .write();
}

#[cfg(test)]
mod tests;
