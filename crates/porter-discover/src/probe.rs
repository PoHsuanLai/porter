//! Probing local ports for a runtime (Ollama, llama.cpp, LM Studio and other OpenAI-compatible
//! servers).
//!
//! Each port is asked, in order, what it is: Ollama's `/api/tags` (then `/api/show` per model for
//! its features and context), llama.cpp's `/props`, then the OpenAI-compatible `/v1/models`. The
//! first that answers decides; a port that refuses or answers nothing readable is skipped.
//! Everything is `127.0.0.1`, plain HTTP: the only plain endpoint porter allows is loopback.

use porter_core::capability::{
    Capability, EmbedCap, EmbedPrompts, LlmCap, LlmFeature, LlmWire, Modality, PrefixText,
};
use porter_core::{Claim, Count, Dims, ModelId, Offer, Provenance, Subject, Tokens, WebUrl};
use porter_http::{Http, HttpRequest, Method};
use porter_provider::Port;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The context a model is assumed to have when its runtime does not say (the provider files'
/// floor for a chat model).
const FLOOR_CONTEXT: Tokens = Tokens(2048);

/// The most models read from one runtime.
const MAX_MODELS: usize = 64;

/// A runtime found on a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeHit {
    /// The port.
    pub port: Port,
    /// What it serves, as `Discovered` claims.
    pub claims: Vec<Claim>,
}

/// Asks each of `ports` on 127.0.0.1 what answers, in order.
pub async fn probe_ports<H: Http>(http: &H, ports: &[Port]) -> Vec<ProbeHit> {
    let mut hits = Vec::new();
    for port in ports {
        if let Some(claims) = probe_port(http, *port).await {
            hits.push(ProbeHit {
                port: *port,
                claims,
            });
        }
    }
    hits
}

async fn probe_port<H: Http>(http: &H, port: Port) -> Option<Vec<Claim>> {
    let ask = Ask { http, port };
    if let Some(tags) = ask.get("/api/tags").await {
        return Some(ollama(&ask, &tags).await);
    }
    if let Some(props) = ask.get("/props").await {
        return Some(llama_cpp(&props));
    }
    let models = ask.get("/v1/models").await?;
    Some(openai_models(&models))
}

/// One port's requests.
struct Ask<'a, H> {
    http: &'a H,
    port: Port,
}

impl<H: Http> Ask<'_, H> {
    async fn get(&self, path: &str) -> Option<Value> {
        self.call(Method::Get, path, Vec::new()).await
    }

    async fn post(&self, path: &str, body: &Value) -> Option<Value> {
        self.call(Method::Post, path, body.to_string().into_bytes())
            .await
    }

    /// The JSON a successful answer carries; `None` for a refusal, an error or other text.
    async fn call(&self, method: Method, path: &str, body: Vec<u8>) -> Option<Value> {
        let url = WebUrl::parse(&format!("http://127.0.0.1:{}{path}", self.port.0)).ok()?;
        let request = HttpRequest::new(method, url)
            .with_header("Content-Type", "application/json")
            .with_body(body);
        let response = self.http.send(request).await.ok()?;
        match response.status.is_success() {
            true => serde_json::from_slice(&response.body).ok(),
            false => None,
        }
    }
}

async fn ollama<H: Http>(ask: &Ask<'_, H>, tags: &Value) -> Vec<Claim> {
    let names = tags
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("name").or_else(|| m.get("model")))
        .filter_map(Value::as_str)
        .take(MAX_MODELS);
    let mut claims = Vec::new();
    for name in names {
        let Some(id) = model_id(name) else { continue };
        let show = ask.post("/api/show", &json!({ "model": name })).await;
        claims.extend(ollama_model(id, show.as_ref()));
    }
    claims
}

/// One Ollama model's claims from its `/api/show` answer (`None` when it could not be read: the
/// chat floor).
fn ollama_model(id: ModelId, show: Option<&Value>) -> Vec<Claim> {
    let capabilities: Option<BTreeSet<&str>> = show
        .and_then(|s| s.get("capabilities"))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect());
    let info = show.and_then(|s| s.get("model_info"));
    let arch = info
        .and_then(|i| i.get("general.architecture"))
        .and_then(Value::as_str);
    let number = |suffix: &str| {
        let key = format!("{}.{suffix}", arch?);
        let value = info?.get(&key)?.as_u64()?;
        u32::try_from(value).ok()
    };
    let context = number("context_length").map_or(FLOOR_CONTEXT, Tokens);
    let capable = |name: &str| capabilities.as_ref().is_some_and(|c| c.contains(name));
    let embedding = capable("embedding");
    let chat = capabilities.is_none() || capable("completion");
    let mut claims = Vec::new();
    if chat {
        let features = [
            Some(LlmFeature::Chat),
            capable("tools").then_some(LlmFeature::Tools),
            capable("vision").then_some(LlmFeature::Vision),
            capable("thinking").then_some(LlmFeature::Reasoning),
        ]
        .into_iter()
        .flatten()
        .collect();
        claims.push(model_claim(
            id.clone(),
            Capability::Llm(LlmCap {
                features,
                context,
                max_output: context,
                wire: LlmWire::OllamaNative,
            }),
        ));
    }
    if let (true, Some(dims)) = (embedding, number("embedding_length")) {
        claims.push(model_claim(
            id,
            Capability::Embeddings(EmbedCap {
                dims: Dims(dims),
                modalities: BTreeSet::from([Modality::Text]),
                max_input: context,
                max_batch: Count(1),
                prompts: Box::new(EmbedPrompts {
                    query: PrefixText(String::new()),
                    document: PrefixText(String::new()),
                }),
            }),
        ));
    }
    claims
}

/// llama.cpp's `/props`: one model, its context from `default_generation_settings.n_ctx`.
fn llama_cpp(props: &Value) -> Vec<Claim> {
    let name = ["model_alias", "model_path"]
        .iter()
        .filter_map(|key| props.get(key).and_then(Value::as_str))
        .find(|text| !text.is_empty())
        .map(|text| text.rsplit('/').next().unwrap_or(text));
    let context = props
        .pointer("/default_generation_settings/n_ctx")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .map_or(FLOOR_CONTEXT, Tokens);
    let id = model_id(name.unwrap_or("llama-cpp"));
    id.into_iter()
        .map(|id| chat_claim(id, context, LlmWire::ChatCompletions))
        .collect()
}

/// An OpenAI-compatible `/v1/models` list: chat models of unknown context.
fn openai_models(models: &Value) -> Vec<Claim> {
    models
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .take(MAX_MODELS)
        .filter_map(model_id)
        .map(|id| chat_claim(id, FLOOR_CONTEXT, LlmWire::ChatCompletions))
        .collect()
}

fn chat_claim(id: ModelId, context: Tokens, wire: LlmWire) -> Claim {
    model_claim(
        id,
        Capability::Llm(LlmCap {
            features: BTreeSet::from([LlmFeature::Chat]),
            context,
            max_output: context,
            wire,
        }),
    )
}

fn model_claim(id: ModelId, capability: Capability) -> Claim {
    Claim {
        subject: Subject::Model(id),
        offer: Offer::Present(capability),
        provenance: Provenance::Discovered,
    }
}

/// A runtime's model name as an id: lower case, anything outside `[a-z0-9._-]` becomes `-`
/// (`Llama3.2:3B` is `llama3.2-3b`), cut to the id length.
fn model_id(name: &str) -> Option<ModelId> {
    let slug: String = name
        .to_ascii_lowercase()
        .chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                true => c,
                false => '-',
            },
        )
        .take(64)
        .collect();
    ModelId::parse(&slug).ok()
}

#[cfg(test)]
mod tests;
