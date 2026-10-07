//! An account route: the agent's request goes to the account's provider as it was sent, with the
//! real key in the provider's own header. The key is asked of accountd for this request, on a
//! sealed descriptor, held in the client built for it, and dropped with it; it is in no reply,
//! error, log line, audit line or bus message. What comes back is relayed as it arrives and
//! read for its usage figures, which are counted against the account's and the program's caps.
//!
//! Nothing of the request is mapped: tool calls and results, thinking, images, server tools and
//! cache hints pass as the agent wrote them. The one change is OpenAI streaming, which asks for
//! a usage chunk so the stream can be metered. A provider's error body is never relayed (it may
//! echo the prompt); the agent is told the kind of thing that went wrong, in its own error shape.

use super::fail::{Cause, Failure, Shape};
use super::handle::Peek;
use super::http::{self, Request};
use super::meter::{Line, Reading, assumed, settle};
use super::open::priced;
use super::route::admits;
use super::session::Ctx;
use crate::cloud::models::{known_providers, price_of, provider_of};
use crate::cloud::spend::estimate;
use model_catalog::{ProviderId, Wire};
use model_http::{
    BodySink, ChunkFlow, Exchange, Framing, HttpError, JsonBody, ResponseHead, RouteRoot,
    Transport, UrlPath, Verb,
};
use porter_core::consent::{Usage, Verdict};
use porter_core::{AccountId, Locality, ModelId};
use porter_infer::{ServedBy, SpendVerdict, TokenUsage};
use serde_json::Value;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

/// What the transport hands the relay.
enum Piece {
    Head(ResponseHead),
    Chunk(Vec<u8>),
}

/// The transport's sink: every piece goes down a channel to the relay.
struct Tap(mpsc::UnboundedSender<Piece>);

impl BodySink for Tap {
    fn head(&mut self, head: &ResponseHead) -> ChunkFlow {
        flow(self.0.send(Piece::Head(head.clone())).is_ok())
    }

    fn chunk(&mut self, bytes: &[u8]) -> ChunkFlow {
        flow(self.0.send(Piece::Chunk(bytes.to_vec())).is_ok())
    }
}

fn flow(open: bool) -> ChunkFlow {
    if open {
        ChunkFlow::Continue
    } else {
        ChunkFlow::Stop
    }
}

/// What the relay has done with the reply so far.
struct Relay {
    stream: bool,
    reading: Reading,
    head: Option<ResponseHead>,
    started: bool,
    gone: bool,
    body: Vec<u8>,
}

impl Relay {
    fn accepted(&self) -> bool {
        self.head
            .as_ref()
            .is_some_and(|head| (200..300).contains(&head.status.0))
    }

    async fn take<S: AsyncWrite + Unpin>(&mut self, piece: Piece, out: &mut S) {
        match piece {
            Piece::Head(head) => {
                self.head = Some(head);
                if self.accepted() && self.stream {
                    self.started = true;
                    self.gone |= http::start_stream(out).await.is_err();
                }
            }
            Piece::Chunk(bytes) if self.accepted() => {
                if self.stream {
                    self.reading.feed(&bytes);
                    if !self.gone {
                        self.gone |= http::write_chunk(out, &bytes).await.is_err();
                    }
                } else {
                    self.body.extend_from_slice(&bytes);
                }
            }
            Piece::Chunk(_) => {}
        }
    }
}

/// The failure a provider's non-success status is told to the agent as.
fn upstream(head: Option<&ResponseHead>) -> Failure {
    let status = head.map_or(0, |head| head.status.0);
    match status {
        429 => Failure::new(
            Cause::RateLimited(head.and_then(|h| h.retry_after).map(|w| w.0)),
            "the provider is rate limiting this account",
        ),
        503 | 529 => Failure::new(Cause::Overloaded, "the provider is overloaded"),
        401 | 403 => Failure::new(
            Cause::Upstream,
            "the provider refused the account's credentials",
        ),
        404 => Failure::new(Cause::NotFound, "the provider does not know the model"),
        400 | 413 | 422 => Failure::new(Cause::Invalid, "the provider rejected the request"),
        _ => Failure::new(Cause::Upstream, "the provider could not answer"),
    }
}

/// The body to send: as the agent wrote it, except that an OpenAI stream asks for its usage.
fn body_for(request: &Request, shape: Shape, stream: bool) -> Result<String, Failure> {
    let invalid = || Failure::new(Cause::Invalid, "the body is not JSON text");
    if shape == Shape::OpenAi && stream {
        let mut root: Value = serde_json::from_slice(&request.body).map_err(|_| invalid())?;
        root["stream_options"] = serde_json::json!({ "include_usage": true });
        return Ok(root.to_string());
    }
    String::from_utf8(request.body.clone()).map_err(|_| invalid())
}

/// The headers the Anthropic API wants beside the key: the version the agent asked for, and its
/// beta flags.
fn anthropic_headers(request: &Request) -> Vec<(String, String)> {
    let version = request.header("anthropic-version").unwrap_or("2023-06-01");
    let mut headers = vec![("anthropic-version".to_owned(), version.to_owned())];
    if let Some(beta) = request.header("anthropic-beta") {
        headers.push(("anthropic-beta".to_owned(), beta.to_owned()));
    }
    headers
}

fn wire_of(shape: Shape) -> Wire {
    match shape {
        Shape::Anthropic => Wire::AnthropicMessages,
        Shape::OpenAi => Wire::OpenAiCompat,
    }
}

/// The provider of `account` when the program holds a grant on it now, and the grant.
async fn granted(
    ctx: &Ctx,
    account: &AccountId,
) -> Result<(ProviderId, porter_core::GrantId), Failure> {
    let cloud = ctx
        .engines
        .cloud()
        .ok_or_else(|| Failure::new(Cause::Upstream, "this daemon has no hosted models"))?;
    let verdicts = cloud
        .accounts(&ctx.app, ctx.class, Usage::Interactive)
        .await;
    let forbidden = || {
        Failure::new(
            Cause::Forbidden,
            "the account is not granted to this program for this class",
        )
    };
    let one = verdicts
        .iter()
        .find(|one| one.account == *account)
        .ok_or_else(forbidden)?;
    let Verdict::Granted { grant, .. } = &one.verdict else {
        return Err(forbidden());
    };
    let provider = provider_of(one, &known_providers(cloud.entries()))
        .ok_or_else(|| Failure::new(Cause::Upstream, "the account is of no known provider"))?;
    Ok((provider, grant.clone()))
}

/// The model ids the account's provider prices (an any-model route lists these).
pub async fn priced_ids(ctx: &Ctx, account: &AccountId) -> Vec<String> {
    let (Ok((provider, _)), Some(cloud)) = (granted(ctx, account).await, ctx.engines.cloud())
    else {
        return Vec::new();
    };
    let mut ids: Vec<String> = cloud
        .entries()
        .iter()
        .filter_map(|entry| match &entry.locality {
            model_catalog::Locality::Remote { reach } => Some(reach),
            model_catalog::Locality::OnDevice => None,
        })
        .flatten()
        .filter(|reach| reach.provider == provider)
        .map(|reach| reach.model.0.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Forwards `request` to the account's provider and relays the reply.
pub async fn run<S: AsyncWrite + Unpin + Send>(
    ctx: &Ctx,
    account: &AccountId,
    shape: Shape,
    request: &Request,
    named: &Peek,
    out: &mut S,
) -> Result<(), Failure> {
    let cloud = ctx
        .engines
        .cloud()
        .ok_or_else(|| Failure::new(Cause::Upstream, "this daemon has no hosted models"))?;
    let settings = ctx.engines.settings();
    if !admits(&settings, ctx.class, &Locality::Cloud { region: None }) {
        return Err(Failure::new(
            Cause::Forbidden,
            "the policy keeps data of this class off the cloud",
        ));
    }
    let (provider, grant) = granted(ctx, account).await?;
    let (entry, reach) = priced(cloud.entries(), &provider, &named.model).ok_or_else(|| {
        Failure::new(
            Cause::NotFound,
            format!("the account's provider prices no model {}", named.model),
        )
    })?;
    if reach.wire != wire_of(shape) {
        return Err(Failure::new(
            Cause::Invalid,
            "this route's provider speaks the other protocol; open the endpoint for that one",
        ));
    }
    let price = price_of(reach);
    let verdict = cloud.ledger().verdict(
        settings.spend,
        &ctx.app,
        account,
        estimate(&price),
        cloud.now(),
    );
    if verdict == SpendVerdict::Stop {
        return Err(Failure::new(
            Cause::SpendCap,
            "a spend cap for this account or this agent is reached",
        ));
    }
    let model_id = ModelId::parse(&entry.id.0)
        .map_err(|_| Failure::new(Cause::Upstream, "the catalogue entry has no usable id"))?;
    let body = body_for(request, shape, named.stream)?;
    let sent = u64::try_from(body.len()).unwrap_or(u64::MAX);
    let key = cloud
        .key(&grant)
        .await
        .map_err(|_| Failure::new(Cause::Upstream, "the account's key could not be read"))?;
    let extra = match shape {
        Shape::Anthropic => anthropic_headers(request),
        Shape::OpenAi => Vec::new(),
    };
    let client = cloud
        .agent_client(&provider, reach.wire, &key, extra)
        .ok_or_else(|| Failure::new(Cause::Upstream, "no address for the account's provider"))?;
    drop(key);
    let exchange = Exchange {
        verb: Verb::PostJson,
        root: RouteRoot::Base,
        path: UrlPath(
            match shape {
                Shape::Anthropic => "/messages",
                Shape::OpenAi => "/chat/completions",
            }
            .to_owned(),
        ),
        body: Some(JsonBody(body)),
        framing: if named.stream {
            Framing::Sse
        } else {
            Framing::Whole
        },
    };
    let (tx, mut pieces) = mpsc::unbounded_channel();
    let mut tap = Tap(tx);
    let mut relay = Relay {
        stream: named.stream,
        reading: if named.stream {
            Reading::of_stream(shape)
        } else {
            Reading::of_body(shape)
        },
        head: None,
        started: false,
        gone: false,
        body: Vec::new(),
    };
    let sending = client.exchange(&exchange, &mut tap);
    tokio::pin!(sending);
    let result = loop {
        tokio::select! {
            done = &mut sending => break done,
            Some(piece) = pieces.recv() => {
                relay.take(piece, out).await;
                if relay.gone {
                    break Err(HttpError::Broken);
                }
            }
        }
    };
    while let Ok(piece) = pieces.try_recv() {
        relay.take(piece, out).await;
    }
    let accepted = relay.accepted();
    if accepted && !named.stream {
        relay.reading.finish_body(&relay.body);
    }
    let usage: TokenUsage = relay.reading.seen().usage().unwrap_or_else(assumed);
    if accepted {
        settle(ctx, cloud, account, model_id, &price, usage, sent);
    } else {
        Line {
            ctx,
            served: ServedBy {
                account: account.clone(),
                model: model_id,
                locality: Locality::Cloud { region: None },
            },
            usage: TokenUsage {
                input: porter_core::Tokens(0),
                output: porter_core::Tokens(0),
                cached: porter_core::Tokens(0),
            },
            cost: None,
            sent,
        }
        .write();
    }
    finish(&relay, result, out, shape).await
}

/// Ends the reply: closes a stream, sends a buffered body, or tells the failure.
async fn finish<S: AsyncWrite + Unpin>(
    relay: &Relay,
    result: Result<model_http::HttpStatus, HttpError>,
    out: &mut S,
    _shape: Shape,
) -> Result<(), Failure> {
    if relay.gone {
        return Ok(());
    }
    if relay.accepted() {
        if relay.started {
            let _ = http::end_stream(out).await;
            return Ok(());
        }
        return match result {
            Ok(status) => {
                http::write_json(out, status.0, &[], &String::from_utf8_lossy(&relay.body))
                    .await
                    .map_err(|_| Failure::new(Cause::Upstream, "the client went away"))
            }
            Err(_) => Err(Failure::new(
                Cause::Upstream,
                "the provider's reply was cut off",
            )),
        };
    }
    match relay.head {
        Some(_) => Err(upstream(relay.head.as_ref())),
        None => Err(Failure::new(
            Cause::Upstream,
            "the provider could not be reached",
        )),
    }
}

#[cfg(test)]
mod tests;
