//! `inferd.toml` and the directories: where the catalog, the sockets, the audit file and the
//! configuration are. The engine programs and the caller table come from the file, never from the
//! environment; the directories come from the XDG variables (injected, so a test names its own).

use crate::catalog::CatalogDirs;
use crate::local::EngineConfig;
use crate::peers::CallerTable;
use crate::probe::ProbeConfig;
use crate::structured::AiConfig;
use porter_infer::{Policy, TierMap};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Why the configuration could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The file is not valid TOML of the expected shape.
    #[error("{0}")]
    Toml(String),
    /// An `[engines.attached."<id>"]` table the checks of the file refuse.
    #[error("{0}")]
    Attached(#[from] crate::attached::AttachedError),
    /// Neither `$XDG_RUNTIME_DIR` nor a home to place the sockets in.
    #[error("no $XDG_RUNTIME_DIR and no $HOME")]
    NoDirs,
}

/// The file's contents. Every table is optional; a file that does not exist is the default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferdConfig {
    /// Engine programs and the weights cache.
    #[serde(default)]
    pub engines: EngineConfig,
    /// The old shape of the policy, a `[policy]` table. Still read (one way: the rows at their
    /// settings paths under `[ai]` win over it, and a write by Settings goes there); the design's
    /// proposed defaults when absent. Goes when FINDINGS "inferd.toml: the old [policy] and
    /// [tiers] tables" says.
    #[serde(default)]
    pub policy: Option<Policy>,
    /// The old shape of the tier map, `[[tiers.rows]]`: read the same way as `policy`.
    #[serde(default)]
    pub tiers: TierMap,
    /// Which executable is which caller.
    #[serde(default)]
    pub callers: CallerTable,
    /// The `ai.*` settings rows, at their paths.
    #[serde(default)]
    pub ai: AiConfig,
    /// Which ports are probed for the runtimes the person runs themselves (Ollama, llama.cpp,
    /// LM Studio), and how often. Left out of the file while it is the default.
    #[serde(default, skip_serializing_if = "ProbeConfig::is_default")]
    pub probe: ProbeConfig,
}

impl InferdConfig {
    /// The configuration in `text`. An attached engine that is named wrongly (no `where`, a
    /// `url` that is not loopback, both a socket and a url) refuses the whole file.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text).map_err(|e| ConfigError::Toml(e.to_string()))?;
        config.engines.attached()?;
        Ok(config)
    }

    /// The policy of the old `[policy]` table, or the proposed one; the `ai.*` rows are laid over
    /// it by [`crate::settings::resolve`].
    pub fn policy(&self) -> Policy {
        self.policy.clone().unwrap_or_else(Policy::proposed)
    }
}

/// Where everything is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// `$XDG_CONFIG_HOME/quire/inferd.toml`.
    pub config: PathBuf,
    /// The catalog directories.
    pub catalog: CatalogDirs,
    /// `$XDG_RUNTIME_DIR/inferd`: the engines' sockets.
    pub sockets: PathBuf,
    /// `$XDG_STATE_HOME/quire/inferd/audit.jsonl`.
    pub audit: PathBuf,
    /// `$XDG_STATE_HOME/quire/inferd/spend.json`: what hosted models have cost this day and month.
    pub spend: PathBuf,
    /// `$HF_HOME/hub`, else `~/.cache/huggingface/hub`: the weights, when the file names none.
    pub hf_cache: PathBuf,
}

impl Dirs {
    /// The directories for the variables `var` answers (`HOME`, `XDG_CONFIG_HOME`,
    /// `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_RUNTIME_DIR`, `HF_HOME`).
    pub fn from_vars(var: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let home = var("HOME").map(PathBuf::from);
        let under_home = |dir: &str| home.as_ref().map(|home| home.join(dir));
        let xdg = |name: &str, fallback: &str| {
            var(name)
                .map(PathBuf::from)
                .or_else(|| under_home(fallback))
                .ok_or(ConfigError::NoDirs)
        };
        let config = xdg("XDG_CONFIG_HOME", ".config")?;
        let data = xdg("XDG_DATA_HOME", ".local/share")?;
        let state = xdg("XDG_STATE_HOME", ".local/state")?;
        let runtime = var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or(ConfigError::NoDirs)?;
        let hf_cache = var("HF_HOME")
            .map(|hf| PathBuf::from(hf).join("hub"))
            .or_else(|| under_home(".cache/huggingface/hub"))
            .ok_or(ConfigError::NoDirs)?;
        Ok(Self {
            config: config.join("quire").join("inferd.toml"),
            catalog: CatalogDirs {
                system: PathBuf::from("/usr/share/stoker/catalog"),
                user: data.join("stoker").join("catalog"),
            },
            sockets: runtime.join("inferd"),
            audit: state.join("quire").join("inferd").join("audit.jsonl"),
            spend: state.join("quire").join("inferd").join("spend.json"),
            hf_cache,
        })
    }

    /// The directories of the process's environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(|name| std::env::var(name).ok())
    }
}

impl InferdConfig {
    /// The engines' weights cache: the file's, or the default.
    pub fn engines_in(&self, dirs: &Dirs) -> EngineConfig {
        let mut engines = self.engines.clone();
        if engines.hf_cache.as_os_str().is_empty() {
            engines.hf_cache = dirs.hf_cache.clone();
        }
        engines
    }
}

#[cfg(test)]
mod tests;
