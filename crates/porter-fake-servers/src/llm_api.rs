//! A fake AI company's key check: `GET <prefix>/models` (OpenRouter: `<prefix>/key`), answered `200` with a small list only
//! when the request presents a live key the way that company documents, and `401` otherwise (a
//! right key in the wrong header is as refused as a wrong key). Plain HTTP on loopback, built
//! from the shipped provider file of the company (`rewrite`).

use crate::http::{Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use crate::shipped::point;
use porter_core::Family;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use serde_json::json;
use std::collections::HashSet;
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

/// How a company wants the key presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// Anthropic: `x-api-key` and `anthropic-version`; the list is `{"data": [...]}` at
    /// `/v1/models`.
    XApiKey,
    /// Gemini: `x-goog-api-key`; the list is `{"models": [...]}` at `/v1beta/models`.
    XGoogApiKey,
    /// OpenAI and Moonshot: `Authorization: Bearer`; the list is `{"data": [...]}` at
    /// `/v1/models`.
    Bearer,
    /// OpenRouter: `Authorization: Bearer` at `/v1/key` (its models list is public); the answer
    /// is `{"data": {...}}`.
    BearerKey,
}

impl Auth {
    fn prefix(self) -> &'static str {
        match self {
            Auth::XGoogApiKey => "/v1beta",
            Auth::XApiKey | Auth::Bearer | Auth::BearerKey => "/v1",
        }
    }

    fn family(self) -> Family {
        match self {
            Auth::XApiKey => Family::Messages,
            Auth::XGoogApiKey => Family::GenerateContent,
            Auth::Bearer | Auth::BearerKey => Family::ChatCompletions,
        }
    }
}

/// What the check received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Path and query.
    pub target: String,
    /// The `x-api-key` header, if sent.
    pub x_api_key: Option<String>,
    /// The `anthropic-version` header, if sent.
    pub anthropic_version: Option<String>,
    /// The `x-goog-api-key` header, if sent.
    pub x_goog_api_key: Option<String>,
    /// The `Authorization` header, if sent.
    pub authorization: Option<String>,
    /// The status sent back.
    pub status: u16,
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    auth: Auth,
    keys: Arc<Mutex<HashSet<String>>>,
    calls: Seen<Call>,
}

/// The test's side of a running fake.
#[derive(Debug, Clone)]
pub struct LlmApiHandle {
    shared: Shared,
}

/// The fake, bound and ready to serve.
#[derive(Debug)]
pub struct FakeLlmApi {
    listener: Listener,
    shared: Shared,
}

impl FakeLlmApi {
    /// Binds on loopback a company that wants its key presented as `auth`.
    pub async fn bind(auth: Auth) -> io::Result<Self> {
        let listener = Listener::bind(&Bind::Loopback, "llm-api").await?;
        let port = crate::net::port_of(listener.address());
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                auth,
                keys: Arc::default(),
                calls: Seen::default(),
            },
        })
    }

    /// The handle onto this fake.
    pub fn handle(&self) -> LlmApiHandle {
        LlmApiHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start(auth: Auth) -> io::Result<Running<LlmApiHandle>> {
        let fake = Self::bind(auth).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl LlmApiHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// The API root a provider file names, with the company's version prefix.
    pub fn api_url(&self) -> String {
        format!("{}{}", self.shared.base, self.shared.auth.prefix())
    }

    /// `spec` with its chat endpoint pointed at this fake.
    pub fn point(&self, spec: &ProviderSpec) -> ProviderSpec {
        point(spec, self.shared.auth.family(), &self.api_url())
    }

    /// Makes `key` a live key.
    pub fn seed_key(&self, key: &str) {
        lock(&self.shared.keys).insert(key.to_owned());
    }

    /// Withdraws `key`, as if the person deleted it at the company.
    pub fn delete_key(&self, key: &str) {
        lock(&self.shared.keys).remove(key);
    }

    /// Every request answered, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.shared.calls.all()
    }
}

impl FakeServer for FakeLlmApi {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::ModelList
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        LlmApiHandle {
            shared: self.shared.clone(),
        }
        .point(spec)
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let shared = self.shared;
        serve(
            self.listener,
            None,
            Arc::new(move |request| {
                let response = answer(&shared, &request);
                shared.calls.push(Call {
                    target: request.target.clone(),
                    x_api_key: request.header("x-api-key").map(str::to_owned),
                    anthropic_version: request.header("anthropic-version").map(str::to_owned),
                    x_goog_api_key: request.header("x-goog-api-key").map(str::to_owned),
                    authorization: request.header("authorization").map(str::to_owned),
                    status: response.status,
                });
                response
            }),
        )
    }
}

fn answer(shared: &Shared, request: &Request) -> Response {
    let leaf = match shared.auth {
        Auth::BearerKey => "key",
        _ => "models",
    };
    let wanted = format!("{}/{leaf}", shared.auth.prefix());
    if request.method != "GET" || request.path() != wanted {
        return Response::new(404);
    }
    let presented = match shared.auth {
        Auth::XApiKey => request
            .header("anthropic-version")
            .and_then(|_| request.header("x-api-key")),
        Auth::XGoogApiKey => request.header("x-goog-api-key"),
        Auth::Bearer | Auth::BearerKey => request.bearer(),
    };
    let live = presented.is_some_and(|key| lock(&shared.keys).contains(key));
    if !live {
        return Response::json(401, &json!({ "error": { "message": "invalid api key" } }));
    }
    match shared.auth {
        Auth::XGoogApiKey => Response::json(
            200,
            &json!({ "models": [{ "name": "models/gemini-fake", "displayName": "Gemini fake" }] }),
        ),
        Auth::BearerKey => Response::json(
            200,
            &json!({ "data": { "label": "sk-or-v1-...fake", "limit": null, "usage": 0 } }),
        ),
        Auth::XApiKey | Auth::Bearer => Response::json(
            200,
            &json!({ "object": "list", "data": [{ "id": "fake-model", "object": "model" }] }),
        ),
    }
}
