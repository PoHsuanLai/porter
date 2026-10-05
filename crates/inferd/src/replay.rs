//! The replay engine: an engine kind that exists only when `inferd.toml` names it,
//!
//! ```toml
//! [engines.scripted]
//! replay = "/path/to/cassette.jsonl"
//! ```
//!
//! It answers the chat route of an OpenAI-compatible engine from a cassette of (request match,
//! reply) pairs, so the real inferd binary can run where no model can: in an acceptance run, on a
//! machine with no GPU. The cassette's path comes from this table and from nowhere else (no
//! environment, no search path). Nothing here opens the network; a file that is missing or
//! malformed refuses the engine's start, and a request no entry answers gets an HTTP error whose
//! `type` is `replay_miss`.
//!
//! The pieces: [`cassette`] (the file and the match), [`replayer`] (the cassette and its cursor),
//! [`render`] (the reply as wire bytes), [`engine`] (the server on the engine's socket), [`host`]
//! (an `EngineHost` that plays these engines in process and hands the rest on), [`model`] (the
//! catalog entry and `LocalModel` of one).

pub mod cassette;
pub mod engine;
pub mod host;
pub mod model;
pub mod record;
pub mod render;
pub mod replayer;

use crate::local::LocalModel;
use engine_supervisor::{EngineId, SupervisorConfig};
use host::{ReplayHost, Replaying};
use model_catalog::MiB;
use model_provider::Tokens;
use replayer::Replayer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What `[engines.<name>]` says of a replay engine.
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

/// The context a replay model claims when its cassette cannot be read.
const FALLBACK_CONTEXT: Tokens = Tokens(8192);

/// The replay engines of a configuration: their models, and what their host needs.
#[derive(Debug, Clone, Default)]
pub struct Replays {
    /// One model per engine whose name is a model id.
    pub models: Vec<LocalModel>,
    /// The engines the host plays.
    pub engines: BTreeMap<EngineId, Replaying>,
    /// Engines left out, with the reason, for the log.
    pub skipped: Vec<String>,
}

impl Replays {
    /// Reads every named cassette (a failure is kept in the engine, which then refuses to
    /// start) and makes the models, with sockets under `sockets`.
    pub fn read(named: &BTreeMap<String, NamedEngine>, sockets: &Path) -> Self {
        named
            .iter()
            .fold(Self::default(), |mut replays, (name, engine)| {
                let replayer = Replayer::load(&engine.replay).map(Arc::new);
                let context = replayer.as_ref().map_or(FALLBACK_CONTEXT, |played| {
                    played.cassette().header.context.loaded
                });
                match model::model(name, &engine.replay, context, sockets) {
                    Some(model) => {
                        replays.engines.insert(
                            model.spec.id.clone(),
                            Replaying {
                                socket: model.socket.0.clone(),
                                replayer,
                                record: engine.record.clone().map(record::Recorder::new),
                            },
                        );
                        replays.models.push(model);
                    }
                    None => replays.skipped.push(format!("{name}: not a model id")),
                }
                replays
            })
    }

    /// The problems of the cassettes that did not read, for the log.
    pub fn unread(&self) -> Vec<String> {
        self.engines
            .iter()
            .filter_map(|(id, replaying)| {
                replaying
                    .replayer
                    .as_ref()
                    .err()
                    .map(|why| format!("{}: {why}", id.0))
            })
            .collect()
    }

    /// The supervisor's settings for a daemon of `total` models. When every model is a replay
    /// engine nothing takes GPU memory, so the headroom kept free for a real engine's growth is
    /// zero and a computer with no GPU (no `nvidia-smi`) can still start them. With any real
    /// engine among the models the default stands. The GPU is reported as it is either way.
    pub fn supervisor_config(&self, total: usize) -> SupervisorConfig {
        let only_replay = total == self.models.len();
        match only_replay && !self.models.is_empty() {
            true => SupervisorConfig {
                headroom: MiB(0),
                ..SupervisorConfig::default()
            },
            false => SupervisorConfig::default(),
        }
    }

    /// The host that plays these engines and hands every other to `inner`.
    pub fn host<H>(&self, inner: H) -> ReplayHost<H> {
        ReplayHost::new(inner, self.engines.clone())
    }
}
