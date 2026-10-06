//! Probing the runtimes the person runs themselves: Ollama on `127.0.0.1:11434`, a llama.cpp
//! server on `:8080`, LM Studio on `:1234` (design/31 §3.3). The ports are configuration (the
//! `[probe]` table of `inferd.toml`, these defaults); the host is always `127.0.0.1`, the only
//! plain-HTTP address porter allows.
//!
//! What a port answers is read by `porter_discover::probe_ports` (Ollama's `/api/tags` and
//! `/api/show`, llama.cpp's `/props`, an OpenAI-compatible `/v1/models`), which gives each model
//! as a `Discovered` claim under a model id. A chat request needs the name the runtime itself
//! uses (`llama3.2:3b`, where the id is `llama3.2-3b`), so the runtime's list is read once more
//! for names and each name is paired with the claim whose id it makes ([`names`]).
//!
//! Nothing here reports or serves: [`probe`] answers what is running now, and [`entry`] makes the
//! [`LocalModel`] one found model is.

pub mod entry;
pub mod names;

use crate::local::LocalModel;
use names::{Listing, pair};
use porter_core::{Claim, ModelId, Offer, Subject};
use porter_discover::probe_ports;
use porter_http::Http;
use porter_provider::Port;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// A local runtime inferd knows how to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Runtime {
    /// Ollama, whose list is `/api/tags`.
    Ollama,
    /// A llama.cpp `llama-server`.
    LlamaCpp,
    /// LM Studio's local server.
    LmStudio,
}

impl Runtime {
    /// Every runtime, in the order they are probed.
    pub const ALL: [Runtime; 3] = [Runtime::Ollama, Runtime::LlamaCpp, Runtime::LmStudio];

    /// The provider file of the runtime, which is also the id of its account.
    pub fn provider(self) -> &'static str {
        match self {
            Runtime::Ollama => "ollama",
            Runtime::LlamaCpp => "llama-cpp",
            Runtime::LmStudio => "lm-studio",
        }
    }
}

/// The `[probe]` table: which ports are asked for which runtime, and how often. A runtime with no
/// ports is not probed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeConfig {
    /// Where Ollama may be.
    #[serde(default = "ProbeConfig::ollama_ports")]
    pub ollama: Vec<u16>,
    /// Where a llama.cpp server may be.
    #[serde(default = "ProbeConfig::llama_cpp_ports")]
    pub llama_cpp: Vec<u16>,
    /// Where LM Studio may be.
    #[serde(default = "ProbeConfig::lm_studio_ports")]
    pub lm_studio: Vec<u16>,
    /// Seconds between looks while something changes.
    #[serde(default = "ProbeConfig::base_secs")]
    pub every_s: u64,
    /// Seconds between looks at most, once nothing has changed for a while.
    #[serde(default = "ProbeConfig::longest_secs")]
    pub longest_s: u64,
}

impl ProbeConfig {
    fn ollama_ports() -> Vec<u16> {
        vec![11_434]
    }

    fn llama_cpp_ports() -> Vec<u16> {
        vec![8080]
    }

    fn lm_studio_ports() -> Vec<u16> {
        vec![1234]
    }

    fn base_secs() -> u64 {
        10
    }

    fn longest_secs() -> u64 {
        120
    }

    /// Whether this is the table nobody wrote.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// No port is probed: nothing is asked of this computer.
    pub fn off() -> Self {
        Self {
            ollama: Vec::new(),
            llama_cpp: Vec::new(),
            lm_studio: Vec::new(),
            ..Self::default()
        }
    }

    /// The ports asked for `runtime`.
    pub fn ports(&self, runtime: Runtime) -> &[u16] {
        match runtime {
            Runtime::Ollama => &self.ollama,
            Runtime::LlamaCpp => &self.llama_cpp,
            Runtime::LmStudio => &self.lm_studio,
        }
    }

    /// The shortest wait between two looks.
    pub fn base(&self) -> Duration {
        Duration::from_secs(self.every_s.max(1))
    }

    /// The longest wait between two looks.
    pub fn longest(&self) -> Duration {
        Duration::from_secs(self.longest_s).max(self.base())
    }
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            ollama: Self::ollama_ports(),
            llama_cpp: Self::llama_cpp_ports(),
            lm_studio: Self::lm_studio_ports(),
            every_s: Self::base_secs(),
            longest_s: Self::longest_secs(),
        }
    }
}

/// One model a runtime serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probed {
    /// The id it is known by here.
    pub id: ModelId,
    /// The name the runtime serves it under, which a request names.
    pub name: String,
    /// What it can do, as the probe read it (`Discovered`).
    pub claims: Vec<Claim>,
}

/// A runtime that answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Which runtime.
    pub runtime: Runtime,
    /// The port it answered on.
    pub port: Port,
    /// Its models.
    pub models: Vec<Probed>,
}

impl Found {
    /// Every claim of every model: what is reported to accountd.
    pub fn claims(&self) -> Vec<Claim> {
        self.models
            .iter()
            .flat_map(|model| model.claims.iter().cloned())
            .collect()
    }

    /// The local models these are, with the runtime's port as where each is served.
    pub fn local_models(&self, sockets: &Path) -> Vec<LocalModel> {
        self.models
            .iter()
            .filter_map(|model| entry::local_model(self.runtime, self.port, model, sockets))
            .collect()
    }
}

/// What is running now: for each runtime the first of its ports that answers.
pub async fn probe<H: Http>(http: &H, config: &ProbeConfig) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    for runtime in Runtime::ALL {
        let ports: Vec<Port> = config
            .ports(runtime)
            .iter()
            .map(|port| Port(*port))
            // A port another runtime already answered on is that runtime's.
            .filter(|port| found.iter().all(|one| one.port != *port))
            .collect();
        let hits = probe_ports(http, &ports).await;
        let Some(hit) = hits.into_iter().next() else {
            continue;
        };
        let listing = Listing::read(http, runtime, hit.port).await;
        found.push(Found {
            runtime,
            port: hit.port,
            models: pair(&listing, &hit.claims),
        });
    }
    found
}

/// The model id a claim is about.
pub(crate) fn model_of(claim: &Claim) -> Option<&ModelId> {
    match (&claim.subject, &claim.offer) {
        (Subject::Model(id), Offer::Present(_)) => Some(id),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
