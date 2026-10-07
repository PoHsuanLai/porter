//! A model route: the agent's request, read into inferd's chat request, runs on a model on this
//! computer (a supervised engine, a runtime the person runs, an engine they attached) through
//! the same turn path any app's session uses, and the reply is written back in the protocol the
//! agent spoke. No key, no spend; the class rules apply as they do to any session (a model on
//! another of the person's machines needs `ai.attached.my_network` for on-device-only data).
//!
//! A streamed reply starts when its first answer text or function call arrives: reasoning that
//! comes first is held until then, so a model that only reasons can still be told to the agent
//! as the error it is (`ModelError::OnlyThought`) with a status, not as a stream that ends empty.

use super::fail::{Cause, Failure, Shape};
use super::http;
use super::meter::Line;
use super::open::{local_card, model_ref_of};
use super::route::admits;
use super::session::Ctx;
use super::token::random_hex;
use super::wire::{Parsed, SseOut, only_thought};
use super::{anthropic, openai};
use crate::runner::{Pin, Pinned, Turns};
use crate::serve::{EngineHost, RunningTurn, TurnRunner, TurnStep};
use porter_core::Tokens;
use porter_infer::{
    ChatReply, InferEvent, InferReply, InferRequest, ModelError, Readiness, ServedBy, TokenUsage,
};
use tokio::io::AsyncWrite;

fn no_usage() -> TokenUsage {
    TokenUsage {
        input: Tokens(0),
        output: Tokens(0),
        cached: Tokens(0),
    }
}

/// What a finished turn is: a reply, or the failure to tell the agent.
fn outcome(reply: InferReply) -> Result<ChatReply, Failure> {
    match reply {
        InferReply::Chat(chat) => match only_thought(&chat) {
            Some((stop, thought_len)) => Err(Failure::of_model(ModelError::OnlyThought {
                stop,
                thought_len,
            })),
            None => Ok(chat),
        },
        InferReply::Failed(error) => Err(Failure::of_model(error)),
        InferReply::Refused(_) => Err(Failure::new(
            Cause::Invalid,
            "the model cannot take this request",
        )),
        _ => Err(Failure::new(Cause::Upstream, "the model gave no reply")),
    }
}

fn audit(ctx: &Ctx, served: ServedBy, usage: TokenUsage) {
    Line {
        ctx,
        served,
        usage,
        cost: None,
        sent: 0,
    }
    .write();
}

struct Writing<'a, S> {
    out: &'a mut S,
    sse: Box<dyn SseOut>,
    started: bool,
    held: String,
    gone: bool,
}

impl<S: AsyncWrite + Unpin> Writing<'_, S> {
    async fn send(&mut self, text: String) {
        if !self.gone && !text.is_empty() {
            self.gone |= http::write_chunk(self.out, text.as_bytes()).await.is_err();
        }
    }

    /// Starts the stream, with the reasoning held so far.
    async fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.gone |= http::start_stream(self.out).await.is_err();
        let head = self.sse.begin();
        self.send(head).await;
        if !self.held.is_empty() {
            let held = std::mem::take(&mut self.held);
            let piece = self.sse.thought(&held);
            self.send(piece).await;
        }
    }

    async fn event(&mut self, event: InferEvent) {
        match event {
            InferEvent::ThoughtDelta(text) if self.started => {
                let piece = self.sse.thought(&text);
                self.send(piece).await;
            }
            InferEvent::ThoughtDelta(text) => self.held.push_str(&text),
            InferEvent::TextDelta(text) => {
                self.start().await;
                let piece = self.sse.text(&text);
                self.send(piece).await;
            }
            InferEvent::ToolCall(call) => {
                self.start().await;
                let piece = self.sse.tool(&call);
                self.send(piece).await;
            }
            _ => {}
        }
    }
}

/// Runs the request on the model `id` and writes the reply to `out`.
pub async fn run<S: AsyncWrite + Unpin + Send>(
    ctx: &Ctx,
    id: &str,
    shape: Shape,
    parsed: Parsed,
    out: &mut S,
) -> Result<(), Failure> {
    let engines = &ctx.engines;
    let gone = || Failure::new(Cause::Overloaded, "the model is not available");
    let card = local_card(engines, id).ok_or_else(gone)?;
    if !admits(&engines.settings(), ctx.class, &card.locality) {
        return Err(Failure::new(
            Cause::Forbidden,
            "the policy keeps data of this class off that model",
        ));
    }
    let model_ref = model_ref_of(&card);
    let model = engines.local_model(&model_ref).ok_or_else(gone)?;
    if engines.readiness(&model) == Readiness::Unavailable {
        return Err(gone());
    }
    engines.want(model_ref.clone()).await.map_err(|_| gone())?;
    let served = ServedBy {
        account: card.account.clone(),
        model: card.model.clone(),
        locality: card.locality.clone(),
    };
    let pin = Pin::new();
    pin.set(Pinned {
        served: served.clone(),
        model: Some(model),
        cloud: None,
    });
    let turns = Turns::new(pin, engines.supervised().clone(), parsed.chat.tier);
    let mut turn = turns.start(InferRequest::Chat(parsed.chat), Vec::new());
    let tag = random_hex::<12>().unwrap_or_default();
    let created = ctx.clock.now().0;
    let reply = if parsed.stream {
        let sse: Box<dyn SseOut> = match shape {
            Shape::Anthropic => {
                Box::new(anthropic::Stream::new(&parsed.model, &format!("msg_{tag}")))
            }
            Shape::OpenAi => Box::new(openai::Stream::new(
                &parsed.model,
                &format!("chatcmpl-{tag}"),
                created,
                parsed.include_usage,
            )),
        };
        let mut writing = Writing {
            out,
            sse,
            started: false,
            held: String::new(),
            gone: false,
        };
        let done = loop {
            match turn.next().await {
                TurnStep::Event(event) => {
                    writing.event(event).await;
                    if writing.gone {
                        engines.release(&model_ref);
                        audit(ctx, served, no_usage());
                        return Ok(());
                    }
                }
                TurnStep::Done(reply) => break reply,
            }
        };
        let usage = match &done {
            InferReply::Chat(chat) => chat.usage,
            _ => no_usage(),
        };
        engines.release(&model_ref);
        audit(ctx, served, usage);
        return match outcome(done) {
            Ok(chat) => {
                writing.start().await;
                let last = writing.sse.finish(&chat);
                writing.send(last).await;
                let _ = http::end_stream(writing.out).await;
                Ok(())
            }
            Err(failure) if writing.started => {
                let last = writing.sse.error(&failure);
                writing.send(last).await;
                let _ = http::end_stream(writing.out).await;
                Ok(())
            }
            Err(failure) => Err(failure),
        };
    } else {
        loop {
            if let TurnStep::Done(reply) = turn.next().await {
                break reply;
            }
        }
    };
    engines.release(&model_ref);
    let usage = match &reply {
        InferReply::Chat(chat) => chat.usage,
        _ => no_usage(),
    };
    audit(ctx, served, usage);
    let chat = outcome(reply)?;
    let body = match shape {
        Shape::Anthropic => anthropic::message_json(&parsed.model, &format!("msg_{tag}"), &chat),
        Shape::OpenAi => {
            openai::completion_json(&parsed.model, &format!("chatcmpl-{tag}"), created, &chat)
        }
    };
    http::write_json(out, 200, &[], &body.to_string())
        .await
        .map_err(|_| Failure::new(Cause::Upstream, "the client went away"))
}
