//! One connection: read the request, check the token, route by path, answer.

use super::fail::{Cause, Failure, Shape};
use super::http::{self, ReadError, Request};
use super::route::{Models, Target};
use super::session::Ctx;
use super::wire::Parsed;
use super::{account, anthropic, local, openai};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// The path without the `/v1` an OpenAI base URL carries (and an Anthropic one does not).
fn bare(request: &Request) -> &str {
    let path = request.path();
    path.strip_prefix("/v1").unwrap_or(path)
}

/// The protocol the request speaks, for the shape of its errors.
fn shape_of(request: &Request) -> Shape {
    let path = bare(request);
    let anthropic_models =
        path.starts_with("/models") && request.header("anthropic-version").is_some();
    if path.starts_with("/messages") || anthropic_models {
        Shape::Anthropic
    } else {
        Shape::OpenAi
    }
}

/// What every request names: the model and whether it streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peek {
    /// The id the agent named.
    pub model: String,
    /// Whether it asked for a stream.
    pub stream: bool,
}

/// The model and stream flag of a request body, without reading the rest of it.
pub fn peek(body: &[u8]) -> Result<Peek, Failure> {
    let root: Value = serde_json::from_slice(body)
        .map_err(|_| Failure::new(Cause::Invalid, "the body is not JSON"))?;
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| Failure::new(Cause::Invalid, "the request names no model"))?;
    Ok(Peek {
        model: model.to_owned(),
        stream: root.get("stream").and_then(Value::as_bool) == Some(true),
    })
}

/// Serves one connection and closes it.
pub async fn serve(ctx: Arc<Ctx>, mut stream: TcpStream) {
    let wait = std::time::Duration::from_secs(120);
    let token = &ctx.token;
    // The shape of the protocol the head speaks, kept for the answer to a refused head.
    let speaks_anthropic = std::sync::atomic::AtomicBool::new(false);
    let read = http::read_request(&mut stream, |head| {
        speaks_anthropic.store(
            shape_of(head) == Shape::Anthropic,
            std::sync::atomic::Ordering::Relaxed,
        );
        head.presented().is_some_and(|given| token.matches(given))
    });
    let request = match tokio::time::timeout(wait, read).await {
        Ok(Ok(request)) => request,
        Err(_) | Ok(Err(ReadError::Closed)) => return,
        Ok(Err(ReadError::Refused)) => {
            let failure = Failure::new(Cause::Unauthenticated, "the API key is not this session's");
            let shape = if speaks_anthropic.load(std::sync::atomic::Ordering::Relaxed) {
                Shape::Anthropic
            } else {
                Shape::OpenAi
            };
            let _ = respond(&mut stream, shape, &failure).await;
            return;
        }
        Ok(Err(ReadError::TooLarge)) => {
            let failure = Failure::new(
                Cause::TooLarge,
                "the request is larger than this endpoint reads",
            );
            let _ = respond(&mut stream, Shape::OpenAi, &failure).await;
            return;
        }
        Ok(Err(ReadError::Malformed)) => {
            let failure = Failure::new(Cause::Invalid, "not an HTTP request");
            let _ = respond(&mut stream, Shape::OpenAi, &failure).await;
            return;
        }
    };
    let shape = shape_of(&request);
    if let Err(failure) = dispatch(&ctx, &request, shape, &mut stream).await {
        let _ = respond(&mut stream, shape, &failure).await;
    }
    let _ = stream.shutdown().await;
}

async fn respond<S: AsyncWrite + Unpin>(
    stream: &mut S,
    shape: Shape,
    failure: &Failure,
) -> std::io::Result<()> {
    http::write_json(
        stream,
        failure.status(shape),
        &failure.headers(),
        &failure.body(shape).to_string(),
    )
    .await
}

async fn dispatch<S: AsyncWrite + Unpin + Send>(
    ctx: &Ctx,
    request: &Request,
    shape: Shape,
    out: &mut S,
) -> Result<(), Failure> {
    if !request
        .presented()
        .is_some_and(|given| ctx.token.matches(given))
    {
        return Err(Failure::new(
            Cause::Unauthenticated,
            "the API key is not this session's",
        ));
    }
    match (request.method.as_str(), bare(request)) {
        ("POST", "/messages") => complete(ctx, request, Shape::Anthropic, out).await,
        ("POST", "/chat/completions") => complete(ctx, request, Shape::OpenAi, out).await,
        ("POST", "/messages/count_tokens") => count_tokens(ctx, request, out).await,
        ("GET", "/models") => models(ctx, shape, out).await,
        ("GET", path) if path.starts_with("/models/") => {
            model(ctx, shape, &path["/models/".len()..], out).await
        }
        _ => Err(Failure::new(
            Cause::NotFound,
            "this endpoint has no such path",
        )),
    }
}

/// The ids this route serves, for `/v1/models`: the ones it lists; for an any-model route, the
/// route's own model, or what the account's provider prices.
async fn served_ids(ctx: &Ctx) -> Vec<String> {
    match (&ctx.route.models, &ctx.route.target) {
        (Models::Listed(ids), _) => ids.clone(),
        (Models::Any, Target::Model(id)) => vec![id.clone()],
        (Models::Any, Target::Account(account)) => account::priced_ids(ctx, account).await,
    }
}

fn not_served(id: &str) -> Failure {
    Failure::new(
        Cause::NotFound,
        format!("this route does not serve the model {id}"),
    )
}

/// A chat request: an account route forwards it, a model route runs it.
async fn complete<S: AsyncWrite + Unpin + Send>(
    ctx: &Ctx,
    request: &Request,
    shape: Shape,
    out: &mut S,
) -> Result<(), Failure> {
    let named = peek(&request.body)?;
    if !ctx.route.serves(&named.model) {
        return Err(not_served(&named.model));
    }
    match &ctx.route.target {
        Target::Account(account) => account::run(ctx, account, shape, request, &named, out).await,
        Target::Model(id) => {
            let parsed = parse(shape, request, ctx)?;
            local::run(ctx, id, shape, parsed, out).await
        }
    }
}

fn parse(shape: Shape, request: &Request, ctx: &Ctx) -> Result<Parsed, Failure> {
    let parsed = match shape {
        Shape::Anthropic => anthropic::parse(&request.body, ctx.class),
        Shape::OpenAi => openai::parse(&request.body, ctx.class),
    };
    parsed.map_err(|why| {
        Failure::new(
            Cause::Invalid,
            format!("this route cannot serve the request: {}", why.0),
        )
    })
}

/// `POST /v1/messages/count_tokens`: an estimate (a quarter of the characters), the same for every
/// route. The provider's own count is not asked for, so it costs nothing and counts nothing.
async fn count_tokens<S: AsyncWrite + Unpin>(
    ctx: &Ctx,
    request: &Request,
    out: &mut S,
) -> Result<(), Failure> {
    let named = peek(&request.body)?;
    if !ctx.route.serves(&named.model) {
        return Err(not_served(&named.model));
    }
    let tokens = anthropic::estimate_tokens(&request.body)
        .ok_or_else(|| Failure::new(Cause::Invalid, "the body is not JSON"))?;
    reply_json(out, &json!({ "input_tokens": tokens })).await
}

async fn reply_json<S: AsyncWrite + Unpin>(out: &mut S, body: &Value) -> Result<(), Failure> {
    http::write_json(out, 200, &[], &body.to_string())
        .await
        .map_err(|_| Failure::new(Cause::Upstream, "the client went away"))
}

fn anthropic_model(id: &str) -> Value {
    json!({
        "type": "model",
        "id": id,
        "display_name": id,
        "created_at": "2025-01-01T00:00:00Z",
    })
}

async fn models<S: AsyncWrite + Unpin>(
    ctx: &Ctx,
    shape: Shape,
    out: &mut S,
) -> Result<(), Failure> {
    let ids = served_ids(ctx).await;
    let body = match shape {
        Shape::Anthropic => json!({
            "data": ids.iter().map(|id| anthropic_model(id)).collect::<Vec<_>>(),
            "has_more": false,
            "first_id": ids.first(),
            "last_id": ids.last(),
        }),
        Shape::OpenAi => openai::models_json(&ids, ctx.clock.now().0),
    };
    reply_json(out, &body).await
}

async fn model<S: AsyncWrite + Unpin>(
    ctx: &Ctx,
    shape: Shape,
    id: &str,
    out: &mut S,
) -> Result<(), Failure> {
    if !served_ids(ctx).await.iter().any(|one| one == id) {
        return Err(not_served(id));
    }
    let body = match shape {
        Shape::Anthropic => anthropic_model(id),
        Shape::OpenAi => json!({
            "id": id, "object": "model", "created": ctx.clock.now().0, "owned_by": "porter",
        }),
    };
    reply_json(out, &body).await
}
