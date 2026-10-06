//! Model lists: Ollama's `/api/tags` and `/api/show`, and an OpenAI-compatible `/v1/models`. Both
//! wires also answer `POST /v1/chat/completions` (what a real Ollama, llama.cpp and LM Studio do)
//! with a scripted stream, so a local runtime can be probed and then asked.

use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen, lock};
use crate::shipped::point;
use porter_core::Family;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use serde_json::{Value, json};
use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex};

/// What one model is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDef {
    /// Its name (`llama3.2:3b`).
    pub name: String,
    /// Its architecture family (`llama`).
    pub family: String,
    /// Its context window in tokens.
    pub context: u64,
    /// What it can do (`completion`, `tools`, `embedding`, `vision`).
    pub capabilities: Vec<String>,
}

impl ModelDef {
    /// A chat model with tool calling.
    pub fn chat(name: &str, context: u64) -> Self {
        Self {
            name: name.to_owned(),
            family: "llama".to_owned(),
            context,
            capabilities: vec!["completion".to_owned(), "tools".to_owned()],
        }
    }

    /// An embedding model.
    pub fn embedding(name: &str, context: u64) -> Self {
        Self {
            name: name.to_owned(),
            family: "nomic-bert".to_owned(),
            context,
            capabilities: vec!["embedding".to_owned()],
        }
    }

    fn tag(&self) -> Value {
        json!({
            "name": self.name, "model": self.name, "modified_at": "2026-09-01T10:00:00Z",
            "size": 2_019_393_189_u64, "digest": "a80c4f17acd5",
            "details": { "format": "gguf", "family": self.family, "parameter_size": "3.2B", "quantization_level": "Q4_K_M" },
        })
    }

    fn show(&self) -> Value {
        let mut info = serde_json::Map::new();
        info.insert("general.architecture".to_owned(), json!(self.family));
        info.insert(
            format!("{}.context_length", self.family),
            json!(self.context),
        );
        if self.capabilities.iter().any(|c| c == "embedding") {
            info.insert(format!("{}.embedding_length", self.family), json!(768));
        }
        json!({
            "modelfile": format!("FROM {}", self.name), "template": "{{ .Prompt }}",
            "details": { "format": "gguf", "family": self.family, "parameter_size": "3.2B", "quantization_level": "Q4_K_M" },
            "model_info": info, "capabilities": self.capabilities,
        })
    }
}

/// Which list API a server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// Ollama's native API.
    Ollama,
    /// OpenAI-compatible `/v1/models`, with an optional bearer key required.
    OpenAi,
}

#[derive(Debug, Clone)]
struct Shared {
    base: String,
    wire: Wire,
    models: Arc<Vec<ModelDef>>,
    key: Option<String>,
    hits: Seen<Hit>,
    /// The deltas the chat answer is made of, one per piece.
    says: Arc<Mutex<Vec<String>>>,
    /// The JSON bodies of the chat requests received.
    chats: Seen<Value>,
}

/// The test's side of a running model-list server.
#[derive(Debug, Clone)]
pub struct ModelsHandle {
    shared: Shared,
}

/// The fake model-list server.
#[derive(Debug)]
pub struct FakeModels {
    listener: Listener,
    shared: Shared,
}

impl FakeModels {
    /// Binds a server of this wire holding `models`; `key` (OpenAI wire) is the bearer token it
    /// requires.
    pub async fn bind(wire: Wire, models: Vec<ModelDef>, key: Option<&str>) -> io::Result<Self> {
        Self::bind_on(&Bind::Loopback, wire, models, key).await
    }

    /// `bind`, at `bind` (`Bind::Port` to come back where a stopped server was).
    pub async fn bind_on(
        bind: &Bind,
        wire: Wire,
        models: Vec<ModelDef>,
        key: Option<&str>,
    ) -> io::Result<Self> {
        let listener = Listener::bind(bind, "models").await?;
        let port = crate::net::port_of(listener.address());
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                wire,
                models: Arc::new(models),
                key: key.map(str::to_owned),
                hits: Seen::default(),
                says: Arc::new(Mutex::new(vec!["ok".to_owned()])),
                chats: Seen::default(),
            },
        })
    }

    /// The handle onto this server.
    pub fn handle(&self) -> ModelsHandle {
        ModelsHandle {
            shared: self.shared.clone(),
        }
    }

    /// Binds and serves on a task.
    pub async fn start(
        wire: Wire,
        models: Vec<ModelDef>,
        key: Option<&str>,
    ) -> io::Result<Running<ModelsHandle>> {
        let fake = Self::bind(wire, models, key).await?;
        let handle = fake.handle();
        Ok(Running::spawn(fake, handle))
    }
}

impl ModelsHandle {
    /// `http://127.0.0.1:port`.
    pub fn base_url(&self) -> &str {
        &self.shared.base
    }

    /// Every request answered, oldest first.
    pub fn hits(&self) -> Vec<Hit> {
        self.shared.hits.all()
    }

    /// The JSON bodies of the chat requests received, oldest first.
    pub fn chats(&self) -> Vec<Value> {
        self.shared.chats.all()
    }

    /// What the chat answer says from now on, one delta per piece.
    pub fn say(&self, pieces: &[&str]) {
        *lock(&self.shared.says) = pieces.iter().map(|p| (*p).to_owned()).collect();
    }
}

impl FakeServer for FakeModels {
    fn protocol(&self) -> FakeProtocol {
        FakeProtocol::ModelList
    }

    fn address(&self) -> &FakeAddress {
        self.listener.address()
    }

    fn rewrite(&self, spec: &ProviderSpec) -> ProviderSpec {
        let family = match self.shared.wire {
            Wire::Ollama => Family::OllamaNative,
            Wire::OpenAi => Family::ChatCompletions,
        };
        let url = match self.shared.wire {
            Wire::Ollama => self.shared.base.clone(),
            Wire::OpenAi => format!("{}/v1", self.shared.base),
        };
        point(spec, family, &url)
    }

    fn serve(self) -> impl Future<Output = ()> + Send {
        let shared = self.shared;
        serve(
            self.listener,
            None,
            Arc::new(move |request| {
                let response = answer(&shared, &request);
                shared.hits.push(Hit::of(&request, &response));
                response
            }),
        )
    }
}

fn answer(shared: &Shared, request: &Request) -> Response {
    match (shared.wire, request.method.as_str(), request.path()) {
        (Wire::Ollama, "GET", "/api/tags") => {
            let models: Vec<Value> = shared.models.iter().map(ModelDef::tag).collect();
            Response::json(200, &json!({ "models": models }))
        }
        (Wire::Ollama, "POST", "/api/show") => {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let name = body
                .get("model")
                .or_else(|| body.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            match shared.models.iter().find(|m| m.name == name) {
                Some(model) => Response::json(200, &model.show()),
                None => Response::json(
                    404,
                    &json!({ "error": format!("model '{name}' not found") }),
                ),
            }
        }
        (Wire::OpenAi, "GET", "/v1/models") => {
            let allowed = shared
                .key
                .as_deref()
                .is_none_or(|key| request.bearer() == Some(key));
            if !allowed {
                return Response::json(
                    401,
                    &json!({ "error": { "message": "Incorrect API key provided", "type": "invalid_request_error" } }),
                );
            }
            let data: Vec<Value> = shared
                .models
                .iter()
                .map(|m| json!({ "id": m.name, "object": "model", "created": 1_780_000_000_u64, "owned_by": "fake" }))
                .collect();
            Response::json(200, &json!({ "object": "list", "data": data }))
        }
        (_, "POST", "/v1/chat/completions") => chat(shared, request),
        _ => Response::new(404),
    }
}

/// The scripted stream of a chat request: the pieces as deltas, a stop, usage, and `[DONE]`.
fn chat(shared: &Shared, request: &Request) -> Response {
    let wanted: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let model = wanted.get("model").and_then(Value::as_str).unwrap_or("");
    shared.chats.push(wanted.clone());
    if !shared.models.iter().any(|m| m.name == model) {
        return Response::json(
            404,
            &json!({ "error": { "message": format!("model '{model}' not found"), "type": "not_found" } }),
        );
    }
    let frame = |delta: Value, finish: Value| {
        let body = json!({ "id": "c1", "object": "chat.completion.chunk", "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] });
        format!("data: {body}\n\n")
    };
    let mut body: String = lock(&shared.says)
        .iter()
        .map(|text| frame(json!({ "content": text }), Value::Null))
        .collect();
    body.push_str(&frame(json!({}), json!("stop")));
    body.push_str(
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
    );
    Response::new(200).typed("text/event-stream", body)
}
