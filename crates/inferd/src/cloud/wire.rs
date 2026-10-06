//! How a request to a hosted model is spoken: where each provider is, which dialect of the
//! chat-completions wire it takes, and the two fields stoker's codec cannot say.
//!
//! The codec writes `temperature` on every request and has no `reasoning_effort = "none"`. A
//! hosted model's entry says "no sampling written: the provider's defaults apply" and gpt-6-luna
//! only takes tools with reasoning off, so the transport shapes the body it is given
//! ([`BodyShape`]) rather than the codec being forked.

use model_catalog::ProviderId;
use model_openai_compat::Flavor;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Where one provider's API is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Door {
    /// The host name the certificate must be for.
    pub host: String,
    /// The TCP port.
    pub port: u16,
    /// The path every request goes under (`/v1`).
    pub base: String,
}

/// Where the providers are, by provider id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doors(BTreeMap<ProviderId, Door>);

impl Doors {
    /// The providers' own addresses (the base URLs their documents give).
    pub fn real() -> Self {
        let at = |id: &str, host: &str, base: &str| {
            (
                ProviderId(id.to_owned()),
                Door {
                    host: host.to_owned(),
                    port: 443,
                    base: base.to_owned(),
                },
            )
        };
        Self(BTreeMap::from([
            at("openrouter", "openrouter.ai", "/api/v1"),
            at("openai", "api.openai.com", "/v1"),
            at("moonshot", "api.moonshot.ai", "/v1"),
            at(
                "google-ai",
                "generativelanguage.googleapis.com",
                "/v1beta/openai",
            ),
        ]))
    }

    /// The same, with `provider` at `door` (a test's loopback fake).
    pub fn with(mut self, provider: &str, door: Door) -> Self {
        self.0.insert(ProviderId(provider.to_owned()), door);
        self
    }

    /// Where `provider` is, if this build can speak to it.
    pub fn of(&self, provider: &ProviderId) -> Option<&Door> {
        self.0.get(provider)
    }
}

/// The dialect of the chat-completions wire a provider takes: OpenRouter's own, and OpenAI's
/// standard one (a usage chunk asked for, `reasoning_effort`) for the companies' endpoints.
pub fn flavor_of(provider: &ProviderId) -> Flavor {
    match provider.0.as_str() {
        "openrouter" => Flavor::OpenRouter,
        _ => Flavor::LiteLlm,
    }
}

/// Whether the body keeps the `temperature` the codec writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temperature {
    /// The app chose a sampling: send it.
    Sent,
    /// The app left it to the provider: send none.
    ProviderDefault,
}

/// How reasoning is switched off for a request that carries tools, where a model needs that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoReasoning {
    /// Leave the body as the codec wrote it.
    Untouched,
    /// `"reasoning_effort": "none"` (OpenAI's chat completions).
    EffortField,
    /// `"reasoning": {"effort": "none"}` (OpenRouter).
    ReasoningObject,
}

/// What is changed in a request body after the codec writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyShape {
    /// Whether `temperature` stays.
    pub temperature: Temperature,
    /// Whether reasoning is switched off when the body names tools.
    pub no_reasoning: NoReasoning,
}

impl BodyShape {
    /// The shape for `model` (the entry's id) on `provider`, for a request whose app did or did
    /// not choose a sampling. gpt-6-luna on chat completions takes tools only with reasoning off
    /// (its entry says so).
    pub fn of(provider: &ProviderId, model: &str, temperature: Temperature) -> Self {
        let no_reasoning = match (model, provider.0.as_str()) {
            ("gpt-6-luna", "openrouter") => NoReasoning::ReasoningObject,
            ("gpt-6-luna", _) => NoReasoning::EffortField,
            _ => NoReasoning::Untouched,
        };
        Self {
            temperature,
            no_reasoning,
        }
    }

    /// `body` (a JSON object as text) with the changes made; a body that is not an object is
    /// returned as it came.
    pub fn apply(&self, body: &str) -> String {
        let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(body) else {
            return body.to_owned();
        };
        if self.temperature == Temperature::ProviderDefault {
            fields.remove("temperature");
        }
        let with_tools = fields
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty());
        if with_tools {
            match self.no_reasoning {
                NoReasoning::Untouched => {}
                NoReasoning::EffortField => {
                    fields.insert("reasoning_effort".into(), json!("none"));
                }
                NoReasoning::ReasoningObject => {
                    fields.insert("reasoning".into(), json!({ "effort": "none" }));
                }
            }
        }
        Value::Object(fields).to_string()
    }
}

#[cfg(test)]
mod tests;
