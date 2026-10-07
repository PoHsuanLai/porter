//! The wire to one engine for one turn: stoker's HTTP client (plain to a host, TLS for an https
//! engine, verified against the platform's roots) under the chat-completions codec, retried as
//! inferd retries a local engine.
//!
//! The codec writes `temperature` on every request. An app that chose no sampling left it to the
//! engine's own defaults (a model's tuned ones in Ollama, llama.cpp or LM Studio, a company's for a
//! hosted one), so the body is shaped to carry none: the same two-line shaping inferd does for a
//! hosted model, here for every engine since no catalogue entry says what the sampling should be.

use super::engine::{Engine, KeyUse, Scheme};
use model_http::{
    AuthHeader, BodySink, Exchange, HostName, HttpClient, HttpEndpoint, HttpError, HttpStatus,
    HttpTarget, JsonBody, Port, Proxy, Secret, Timeouts, Transport, UrlPath, WaitMs,
};
use model_openai_compat::OpenAiCodec;
use model_provider as sp;
use model_provider::{Attempt, RetryPolicy, Retrying, Sleeper};
use model_wire::Driver;
use porter_core::SecretText;
use serde_json::Value;
use std::future::Future;
use std::time::Duration;

/// Waits for an engine: the connection, the first byte (a long prompt, a model that loads
/// first, one that thinks first), and between chunks.
const TIMEOUTS: Timeouts = Timeouts {
    connect: WaitMs(10_000),
    first_byte: WaitMs(180_000),
    idle: WaitMs(90_000),
};

/// Three attempts, a quarter of a second doubling to four seconds (inferd's own).
const RETRY: RetryPolicy = RetryPolicy {
    attempts: Attempt(3),
    base: sp::WaitMs(250),
    cap: sp::WaitMs(4000),
};

/// Waits with the clock of the runtime, so a test with paused time does not wait.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TokioSleep;

impl Sleeper for TokioSleep {
    fn sleep(&self, wait: sp::WaitMs) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(Duration::from_millis(u64::from(wait.0)))
    }
}

/// Whether the body keeps the `temperature` the codec writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Temperature {
    /// The app chose a sampling: send it.
    Sent,
    /// The app left it to the engine: send none.
    EngineDefault,
}

/// One turn's connection, its body shaped.
#[derive(Debug, Clone)]
pub(crate) struct Shaped {
    client: HttpClient,
    temperature: Temperature,
}

impl Transport for Shaped {
    fn exchange<K: BodySink>(
        &self,
        ex: &Exchange,
        sink: &mut K,
    ) -> impl Future<Output = Result<HttpStatus, HttpError>> + Send {
        let shaped = Exchange {
            body: ex
                .body
                .as_ref()
                .map(|body| JsonBody(shape(self.temperature, &body.0))),
            ..ex.clone()
        };
        let client = self.client.clone();
        async move { client.exchange(&shaped, sink).await }
    }
}

/// `body` (a JSON object as text) with `temperature` taken out when it is the engine's to
/// choose; a body that is not an object is returned as it came.
pub(crate) fn shape(temperature: Temperature, body: &str) -> String {
    match (temperature, serde_json::from_str::<Value>(body)) {
        (Temperature::EngineDefault, Ok(Value::Object(mut fields))) => {
            fields.remove("temperature");
            Value::Object(fields).to_string()
        }
        _ => body.to_owned(),
    }
}

fn endpoint(engine: &Engine, key: Option<&SecretText>) -> HttpEndpoint {
    let host = HostName(engine.url.host.clone());
    let port = Port(engine.url.port);
    HttpEndpoint {
        target: match engine.url.scheme {
            Scheme::Http => HttpTarget::Tcp { host, port },
            Scheme::Https => HttpTarget::Tls { host, port },
        },
        proxy: Proxy::Direct,
        base: UrlPath(engine.url.base.clone()),
        auth: match (engine.key, key) {
            (KeyUse::Bearer, Some(key)) => AuthHeader::Bearer(Secret(key.expose().to_owned())),
            _ => AuthHeader::None,
        },
        headers: Vec::new(),
        timeouts: TIMEOUTS,
    }
}

/// The provider for one turn on `engine`, authenticated with `key` (copied once into the
/// endpoint's bearer header, whose `Secret` prints nothing, and dropped with the provider).
pub(crate) fn provider(
    engine: &Engine,
    key: Option<&SecretText>,
    temperature: Temperature,
) -> Retrying<Driver<OpenAiCodec, Shaped>, TokioSleep> {
    let shaped = Shaped {
        client: HttpClient::new(endpoint(engine, key)),
        temperature,
    };
    Retrying::new(
        Driver::new(OpenAiCodec::new(engine.dialect.flavor()), shaped),
        RETRY,
        TokioSleep,
    )
}
