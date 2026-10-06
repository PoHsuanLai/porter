//! The real provider files, as the shipped build reads them, and the rewrite that points their
//! endpoints at a fake. A fake never invents a provider: it takes the file the product ships.

use porter_core::Family;
use porter_provider::{Endpoint, ProviderSpec, parse_provider};

/// The shipped `providers/nextcloud.toml`.
pub fn nextcloud() -> ProviderSpec {
    parse(include_str!("../../../providers/nextcloud.toml"))
}

/// The shipped `providers/ollama.toml`.
pub fn ollama() -> ProviderSpec {
    parse(include_str!("../../../providers/ollama.toml"))
}

/// The shipped `providers/google.toml`.
pub fn google() -> ProviderSpec {
    parse(include_str!("../../../providers/google.toml"))
}

/// The shipped `providers/local.toml`.
pub fn local() -> ProviderSpec {
    parse(include_str!("../../../providers/local.toml"))
}

/// The shipped `providers/<id>.toml` of an AI company served by the `api_key` family
/// (`anthropic`, `google-ai`, `moonshot`, `openai`, `openrouter`).
pub fn ai(id: &str) -> ProviderSpec {
    let text = match id {
        "anthropic" => include_str!("../../../providers/anthropic.toml"),
        "google-ai" => include_str!("../../../providers/google-ai.toml"),
        "moonshot" => include_str!("../../../providers/moonshot.toml"),
        "openrouter" => include_str!("../../../providers/openrouter.toml"),
        "openai" => include_str!("../../../providers/openai.toml"),
        other => panic!("no shipped AI provider file `{other}`"),
    };
    parse(text)
}

/// A provider file's text; panics on a bad file (the shipped files are tested elsewhere).
pub fn parse(text: &str) -> ProviderSpec {
    parse_provider(text).unwrap_or_else(|e| panic!("provider file: {e}"))
}

/// `spec` with every capability row of `family` given the fixed endpoint `url`, replacing any
/// it had. Rows of other families are untouched.
pub fn point(spec: &ProviderSpec, family: Family, url: &str) -> ProviderSpec {
    let mut out = spec.clone();
    for row in out
        .capabilities
        .iter_mut()
        .filter(|row| row.family == family)
    {
        row.endpoint = Some(Endpoint(url.to_owned()));
    }
    out
}
