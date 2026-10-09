//! The models this computer can run: one [`LocalModel`] per catalog entry that a configured
//! engine can serve, with its card (what routing sees), the supervisor's spec (how to start its
//! engine) and the socket its engine listens on.
//!
//! Nothing here starts anything. The weights are looked for (a model whose files are not in the
//! cache is `Downloadable`, and there is no downloader yet), and the engine programs come from
//! configuration, never from the environment.

use crate::attached::{Attached, AttachedEntry, AttachedError, Target};
use crate::catalog::claims_of;
use engine_supervisor::{
    EnginePaths, EngineSpec, EnvPair, ProgramPath, SocketPath, UnitSpec, command,
};
use model_catalog::{EngineKind, EngineProfile, ModelEntry};
use model_http::{AuthHeader, HostName, HttpEndpoint, HttpTarget, Port, Proxy, Timeouts, UrlPath};
use model_openai_compat::Flavor;
use model_provider::{Caps, EmbedCaps, ModelName, Tokens};
use porter_core::{AccountId, Billing, Capability, Locality, ModelId, Offer};
use porter_infer::{ModelCard, ModelRef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The account every local model is served under.
pub const LOCAL_ACCOUNT: &str = "local";

/// What `[engines.<name>]` says of a replay engine (an engine that plays a cassette instead of
/// running a program: inferd's `replay` module reads it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedEngine {
    /// The cassette's file.
    pub replay: PathBuf,
    /// A file that gets every request body this engine receives, one JSON line each. This
    /// writes prompts to disk; it exists for acceptance runs and is accepted only here, on a
    /// replay engine (a table with `record` and no `replay` does not parse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<PathBuf>,
}

/// Where the engine programs and the weights are (settings `ai.engine.*`): a kind with no
/// program is not offered. The weights cache is the Hugging Face hub directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Python of the uv environment that has vLLM.
    pub vllm_python: Option<PathBuf>,
    /// The `llama-server` binary.
    pub llama_server: Option<PathBuf>,
    /// quire's speech host.
    pub speech_host: Option<PathBuf>,
    /// The directory of the shared libraries the speech host loads (sherpa-onnx and onnxruntime),
    /// set as the host's `LD_LIBRARY_PATH` and bound read-only into its sandbox.
    #[serde(default)]
    pub speech_host_libs: Option<PathBuf>,
    /// Python of Kokoro-FastAPI's environment.
    pub kokoro_python: Option<PathBuf>,
    /// `<cache>/hub`: where `models--<org>--<name>` directories are. Left out of the file, it
    /// is the default `Dirs` computes.
    #[serde(default)]
    pub hf_cache: PathBuf,
    /// Engines that play a cassette instead of running a program, by name
    /// (`[engines.<name>] replay = "<file>"`).
    #[serde(flatten)]
    pub named: BTreeMap<String, NamedEngine>,
    /// Engines the person already runs and inferd only uses, by catalogue id
    /// (`[engines.attached."<id>"]`, see [`crate::attached`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attached: BTreeMap<String, AttachedEntry>,
}

impl EngineConfig {
    /// The attached engines the file names, each past the checks the file alone allows.
    pub fn attached(&self) -> Result<Vec<Attached>, AttachedError> {
        self.attached
            .iter()
            .map(|(name, entry)| entry.check(name))
            .collect()
    }

    fn program(&self, kind: EngineKind) -> Option<&Path> {
        match kind {
            EngineKind::Vllm => self.vllm_python.as_deref(),
            EngineKind::LlamaServer => self.llama_server.as_deref(),
            EngineKind::SpeechHost => self.speech_host.as_deref(),
            EngineKind::KokoroFastApi => self.kokoro_python.as_deref(),
        }
    }

    fn paths(&self) -> EnginePaths {
        let program = |kind| {
            ProgramPath(
                self.program(kind)
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
            )
        };
        EnginePaths {
            vllm_python: program(EngineKind::Vllm),
            llama_server: program(EngineKind::LlamaServer),
            speech_host: program(EngineKind::SpeechHost),
            kokoro_python: program(EngineKind::KokoroFastApi),
            hf_cache: self.hf_cache.clone(),
        }
    }
}

/// Whether the weights of a model are in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weights {
    /// The repository's cache directory exists.
    Present,
    /// It does not: the model is `Downloadable`.
    Missing,
}

/// One model this computer can run.
#[derive(Debug, Clone)]
pub struct LocalModel {
    /// What routing sees: the account, the model, `OnDevice`, `Free`, its capabilities.
    pub card: ModelCard,
    /// The name the engine serves it under (the catalog id: `command` passes it as the served
    /// model name).
    pub name: ModelName,
    /// The catalog entry.
    pub entry: ModelEntry,
    /// The engine profile chosen.
    pub profile: EngineProfile,
    /// How to start the engine and what it needs.
    pub spec: EngineSpec,
    /// Where the engine listens.
    pub socket: SocketPath,
    /// The wire dialect of the engine, for the chat kinds.
    pub flavor: Option<Flavor>,
    /// The cassette a replay engine plays; its model has no weights to look for.
    pub cassette: Option<PathBuf>,
    /// The loopback port of a runtime the person runs themselves (Ollama, llama.cpp, LM
    /// Studio): the model is reached there over plain HTTP, `socket` is unused, there are no
    /// weights to look for, and nothing here starts or stops anything (`probe`).
    pub loopback: Option<Port>,
    /// Set for an engine the person already runs and attached (`crate::attached`): it is reached
    /// at this target, `socket` and `loopback` are unused for it, there are no weights to look
    /// for, and nothing here starts, stops or evicts it.
    pub attached: Option<Target>,
}

impl LocalModel {
    /// The key of the model in the session machine and the picker.
    pub fn model_ref(&self) -> ModelRef {
        ModelRef {
            account: self.card.account.clone(),
            model: self.card.model.clone(),
        }
    }

    /// Whether the weights are in the cache now: the repository's directory is the one the
    /// engine's sandbox binds, so it is looked for live (a download that finishes makes the
    /// model loadable without a restart).
    pub fn weights(&self) -> Weights {
        if self.cassette.is_some() || self.loopback.is_some() || self.attached.is_some() {
            return Weights::Present;
        }
        match self.spec.unit.sandbox.read.first() {
            Some(dir) if dir.exists() => Weights::Present,
            _ => Weights::Missing,
        }
    }

    /// The chat capabilities of the entry (sampling defaults, context, images, dialect).
    pub fn caps(&self) -> Option<&Caps> {
        self.entry.caps.as_ref()
    }

    /// The embedding capabilities of the entry (width, limits, prefixes), when it embeds.
    pub fn embed(&self) -> Option<&EmbedCaps> {
        self.entry.embed.as_ref()
    }
}

/// An endpoint on a Unix socket with no auth, under the path `base` (`/v1` for a chat engine).
pub fn unix_endpoint(socket: PathBuf, base: &str, timeouts: Timeouts) -> HttpEndpoint {
    HttpEndpoint {
        target: HttpTarget::Unix(socket),
        proxy: Proxy::Direct,
        base: UrlPath(base.to_owned()),
        auth: AuthHeader::None,
        headers: Vec::new(),
        timeouts,
    }
}

/// An endpoint on `127.0.0.1` at `port` over plain HTTP, with no auth: a runtime the person runs.
pub fn loopback_endpoint(port: Port, base: &str, timeouts: Timeouts) -> HttpEndpoint {
    HttpEndpoint {
        target: HttpTarget::Tcp {
            host: HostName("127.0.0.1".to_owned()),
            port,
        },
        proxy: Proxy::Direct,
        base: UrlPath(base.to_owned()),
        auth: AuthHeader::None,
        headers: Vec::new(),
        timeouts,
    }
}

impl LocalModel {
    /// Where this model's engine is reached, under the path `base`: the target the person
    /// attached (with its token read now, or none when the key file is refused, so the engine
    /// answers 401), the loopback port of a runtime they run, or the engine's own socket.
    pub fn endpoint(&self, base: &str, timeouts: Timeouts) -> HttpEndpoint {
        if let Some(target) = &self.attached {
            return target
                .endpoint(base, timeouts)
                .unwrap_or_else(|_| target.unsigned(base, timeouts));
        }
        match self.loopback {
            Some(port) => loopback_endpoint(port, base, timeouts),
            None => unix_endpoint(self.socket.0.clone(), base, timeouts),
        }
    }
}

/// The speech host's unit with its libraries: `LD_LIBRARY_PATH` names the configured directory
/// and the sandbox reads it. Any other engine's unit is as `command` made it.
fn host_libs(mut unit: UnitSpec, kind: EngineKind, engines: &EngineConfig) -> UnitSpec {
    if let (EngineKind::SpeechHost, Some(libs)) = (kind, &engines.speech_host_libs) {
        unit.env.push(EnvPair {
            name: "LD_LIBRARY_PATH".to_owned(),
            value: libs.to_string_lossy().into_owned(),
        });
        unit.sandbox.read.push(libs.clone());
    }
    unit
}

fn kind_slug(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::Vllm => "vllm",
        EngineKind::LlamaServer => "llama_server",
        EngineKind::SpeechHost => "speech_host",
        EngineKind::KokoroFastApi => "kokoro_fast_api",
    }
}

pub(crate) fn flavor_of(kind: EngineKind) -> Option<Flavor> {
    match kind {
        EngineKind::Vllm => Some(Flavor::Vllm),
        EngineKind::LlamaServer => Some(Flavor::LlamaServer),
        EngineKind::SpeechHost | EngineKind::KokoroFastApi => None,
    }
}

/// The models `entries` make on this computer: those with a capability, and an engine profile
/// whose program is configured. The first such profile of an entry is the one used.
/// `sockets` is the directory the engines' sockets go in (`$XDG_RUNTIME_DIR/inferd`).
pub fn build(entries: &[ModelEntry], engines: &EngineConfig, sockets: &Path) -> Vec<LocalModel> {
    entries
        .iter()
        .filter_map(|entry| one(entry, engines, sockets))
        .collect()
}

fn one(entry: &ModelEntry, engines: &EngineConfig, sockets: &Path) -> Option<LocalModel> {
    let model = ModelId::parse(&entry.id.0).ok()?;
    let capabilities: Vec<Capability> = claims_of(entry)
        .into_iter()
        .filter_map(|claim| match claim.offer {
            Offer::Present(capability) => Some(capability),
            Offer::Absent { .. } => None,
        })
        .collect();
    if capabilities.is_empty() {
        return None;
    }
    let profile = entry
        .engines
        .iter()
        .find(|profile| engines.program(profile.kind).is_some())?
        .clone();
    let id = engine_supervisor::EngineId(format!("{}:{}", kind_slug(profile.kind), entry.id.0));
    let socket =
        SocketPath(sockets.join(format!("{}-{}.sock", kind_slug(profile.kind), entry.id.0)));
    let unit = host_libs(
        command(entry, &profile, &engines.paths(), &socket),
        profile.kind,
        engines,
    );
    // An embedding-only entry has no chat context: its longest input is what the cache holds.
    let context = match (&entry.caps, &entry.embed) {
        (Some(caps), _) => caps.context,
        (None, Some(embed)) => embed.max_input,
        (None, None) => Tokens::default(),
    };
    Some(LocalModel {
        card: ModelCard {
            account: AccountId::parse(LOCAL_ACCOUNT).ok()?,
            model,
            locality: Locality::OnDevice,
            billing: Billing::Free,
            capabilities,
        },
        name: ModelName(entry.id.0.clone()),
        spec: EngineSpec {
            id,
            need: entry.vram.need(context),
            unit,
        },
        socket,
        flavor: flavor_of(profile.kind),
        cassette: None,
        loopback: None,
        attached: None,
        entry: entry.clone(),
        profile,
    })
}

#[cfg(test)]
mod tests;
