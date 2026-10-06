//! The names a runtime serves its models under, and which probe claim each belongs to.
//!
//! `porter_discover` reads a model as a claim under an id (`llama3.2:3b` is `llama3.2-3b`: lower
//! case, anything outside `[a-z0-9._-]` a `-`, at most 64 bytes); a request must carry the
//! runtime's own name. The list is read again (`/api/tags` for Ollama, `/v1/models` for the
//! rest) and a name is paired with the claims of the id it makes. A llama.cpp server names its
//! one model by the alias or file in `/props` and by its path in `/v1/models`; when the runtime
//! lists one name and the probe found one model, they are the same model.

use super::{Probed, Runtime, model_of};
use porter_core::{Claim, ModelId, WebUrl};
use porter_http::{Http, HttpRequest, Method};
use porter_provider::Port;
use serde_json::Value;

/// The most names read from one runtime.
const MAX_NAMES: usize = 64;

/// The names one runtime listed, in its order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing(pub Vec<String>);

impl Listing {
    /// Reads the list of `runtime` on `port`; empty when it does not answer.
    pub async fn read<H: Http>(http: &H, runtime: Runtime, port: Port) -> Self {
        let path = match runtime {
            Runtime::Ollama => "/api/tags",
            Runtime::LlamaCpp | Runtime::LmStudio => "/v1/models",
        };
        let Ok(url) = WebUrl::parse(&format!("http://127.0.0.1:{}{path}", port.0)) else {
            return Self::default();
        };
        let Ok(response) = http.send(HttpRequest::new(Method::Get, url)).await else {
            return Self::default();
        };
        match response.status.is_success() {
            true => serde_json::from_slice(&response.body)
                .map(|body| Self::of(runtime, &body))
                .unwrap_or_default(),
            false => Self::default(),
        }
    }

    /// The names in a list answer.
    pub fn of(runtime: Runtime, body: &Value) -> Self {
        let (array, keys): (&str, &[&str]) = match runtime {
            Runtime::Ollama => ("models", &["name", "model"]),
            Runtime::LlamaCpp | Runtime::LmStudio => ("data", &["id"]),
        };
        Self(
            body.get(array)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|row| keys.iter().find_map(|key| row.get(key)?.as_str()))
                .map(str::to_owned)
                .take(MAX_NAMES)
                .collect(),
        )
    }
}

/// A runtime's model name as an id: the rule `porter_discover`'s probe applies.
pub fn id_of(name: &str) -> Option<ModelId> {
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

/// The models of `claims`, each with the name its runtime serves it under. A claim whose id no
/// listed name makes is paired with the only listed name when there is exactly one of each, and
/// is left out otherwise (it could not be asked for).
pub fn pair(listing: &Listing, claims: &[Claim]) -> Vec<Probed> {
    let mut ids: Vec<&ModelId> = Vec::new();
    for id in claims.iter().filter_map(model_of) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let lone = match (ids.as_slice(), listing.0.as_slice()) {
        ([_], [name]) => Some(name.as_str()),
        _ => None,
    };
    ids.into_iter()
        .filter_map(|id| {
            let name = listing
                .0
                .iter()
                .find(|name| id_of(name).as_ref() == Some(id))
                .map(String::as_str)
                .or(lone)?;
            Some(Probed {
                id: id.clone(),
                name: name.to_owned(),
                claims: claims
                    .iter()
                    .filter(|claim| model_of(claim) == Some(id))
                    .cloned()
                    .collect(),
            })
        })
        .collect()
}
