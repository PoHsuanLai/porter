//! What a request cost and who is told: the tokens read from the provider's own usage figures,
//! the price at the reach, one record against the account's and the program's caps, and one audit
//! line. The line holds the program (as its app), the route, the tokens, the cost and the class;
//! never a prompt, a reply, a key or the token.

use super::fail::Shape;
use super::session::Ctx;
use crate::audit::AuditOut;
use model_http::SseDecoder;
use porter_core::{AccountId, Bytes, Count, Locality, MicroUsd, ModelId, PriceTable, Tokens};
use porter_infer::{AuditEntry, ServedBy, TokenUsage, Why, cost};
use serde_json::Value;

/// A provider's usage figures as they arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Seen {
    input: Option<u32>,
    output: Option<u32>,
    cached: Option<u32>,
}

impl Seen {
    /// The figures, when the provider gave any.
    pub fn usage(self) -> Option<TokenUsage> {
        (self.input.is_some() || self.output.is_some()).then(|| TokenUsage {
            input: Tokens(self.input.unwrap_or(0)),
            output: Tokens(self.output.unwrap_or(0)),
            cached: Tokens(self.cached.unwrap_or(0)),
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
    seen: Seen,
    events: Option<SseDecoder>,
}

impl Reading {
    /// A reader of a stream of server-sent events.
    pub fn of_stream(shape: Shape) -> Self {
        Self {
            shape,
            seen: Seen::default(),
            events: Some(SseDecoder::new()),
        }
    }

    /// A reader of one JSON body, which `finish_body` is given whole.
    pub fn of_body(shape: Shape) -> Self {
        Self {
            shape,
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
            if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
                self.seen.take(self.shape, &value);
            }
        }
    }

    /// Reads the whole body of a non-streaming reply.
    pub fn finish_body(&mut self, body: &[u8]) {
        if let Ok(value) = serde_json::from_slice::<Value>(body) {
            self.seen.take(self.shape, &value);
        }
    }

    /// What was seen.
    pub fn seen(&self) -> Seen {
        self.seen
    }
}

/// What a request used when the provider gave no figures: the figure the check before it
/// assumed, so a missing figure never makes a request free.
pub fn assumed() -> TokenUsage {
    TokenUsage {
        input: Tokens(1_000),
        output: Tokens(1_000),
        cached: Tokens(0),
    }
}

/// One audit line for a request.
#[derive(Debug)]
pub struct Line<'a> {
    /// The session.
    pub ctx: &'a Ctx,
    /// The account, the model and where it ran.
    pub served: ServedBy,
    /// The tokens.
    pub usage: TokenUsage,
    /// The cost, when the request was metered.
    pub cost: Option<MicroUsd>,
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
            cost: self.cost,
        });
    }
}

/// Counts a request against the account's and the program's caps, and writes its audit line.
pub fn settle(
    ctx: &Ctx,
    cloud: &crate::cloud::Cloud,
    account: &AccountId,
    model: ModelId,
    price: &PriceTable,
    usage: TokenUsage,
    sent: u64,
) {
    let spent = cost(usage, price);
    cloud
        .ledger()
        .record(&ctx.app, account, usage, spent, cloud.now());
    Line {
        ctx,
        served: ServedBy {
            account: account.clone(),
            model,
            locality: Locality::Cloud { region: None },
        },
        usage,
        cost: Some(spent),
        sent,
    }
    .write();
}

#[cfg(test)]
mod tests;
