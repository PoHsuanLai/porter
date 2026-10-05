//! Model lists: Ollama's `/api/tags` and `/api/show`, and an OpenAI-compatible `/v1/models`.

use crate::http::{Hit, Request, Response, serve};
use crate::net::{Bind, Listener};
use crate::seen::{Running, Seen};
use crate::shipped::point;
use porter_core::Family;
use porter_fake::{FakeAddress, FakeProtocol, FakeServer};
use porter_provider::ProviderSpec;
use serde_json::{Value, json};
use std::future::Future;
use std::io;
use std::sync::Arc;

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
        let listener = Listener::bind(&Bind::Loopback, "models").await?;
        let port = crate::net::port_of(listener.address());
        Ok(Self {
            listener,
            shared: Shared {
                base: format!("http://127.0.0.1:{port}"),
                wire,
                models: Arc::new(models),
                key: key.map(str::to_owned),
                hits: Seen::default(),
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
        _ => Response::new(404),
    }
}
