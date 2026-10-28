//! The engine programs this computer can run models with, by the names the picker and the bus
//! give them: which slots each serves and whether its program is there.

use model_catalog::EngineKind;
use porter_dbus::inference::EngineState;
use porter_infer::Slot;
use porter_router::local::EngineConfig;
use std::path::PathBuf;

/// A program that runs models on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// `llama-server`.
    LlamaCpp,
    /// vLLM's Python.
    Vllm,
    /// Strata, run from its own folder.
    Strata,
    /// quire's speech host.
    Speech,
    /// Kokoro's Python.
    Kokoro,
}

impl Engine {
    pub(super) const ALL: [Engine; 5] = [
        Self::LlamaCpp,
        Self::Vllm,
        Self::Strata,
        Self::Speech,
        Self::Kokoro,
    ];

    /// The engine a catalogue profile of `kind` starts.
    pub(crate) fn of(kind: EngineKind) -> Self {
        match kind {
            EngineKind::LlamaServer => Self::LlamaCpp,
            EngineKind::Vllm => Self::Vllm,
            EngineKind::SpeechHost => Self::Speech,
            EngineKind::KokoroFastApi => Self::Kokoro,
            EngineKind::Strata => Self::Strata,
        }
    }

    /// The engine's stable id on the bus.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama_cpp",
            Self::Vllm => "vllm",
            Self::Strata => "strata",
            Self::Speech => "speech",
            Self::Kokoro => "kokoro",
        }
    }

    /// What the person calls it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama.cpp",
            Self::Vllm => "vLLM",
            Self::Strata => "Strata",
            Self::Speech => "the speech engine",
            Self::Kokoro => "Kokoro",
        }
    }

    /// Whether a model of `slot` can run on it.
    pub(super) fn serves(self, slot: Slot) -> bool {
        let speech = matches!(slot, Slot::VoiceIn | Slot::VoiceOut);
        match self {
            Self::LlamaCpp | Self::Vllm | Self::Strata => !speech,
            Self::Speech => speech,
            Self::Kokoro => slot == Slot::VoiceOut,
        }
    }

    /// Whether its program is there: named in `config` (the programs already resolved by
    /// [`crate::config::ProgramSearch`]) and present. Strata's is the Python of the folder the
    /// person pointed at.
    pub(super) fn state(self, config: &EngineConfig) -> EngineState {
        let program: Option<PathBuf> = match self {
            Self::LlamaCpp => config.llama_server.clone(),
            Self::Vllm => config.vllm_python.clone(),
            Self::Strata => config
                .strata_folder
                .as_deref()
                .and_then(porter_engines::pointed_strata)
                .map(|found| found.program),
            Self::Speech => config.speech_host.clone(),
            Self::Kokoro => config.kokoro_python.clone(),
        };
        match program.filter(|path| path.exists()) {
            Some(path) => EngineState::Found(path),
            None => EngineState::Missing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogue_engine_kind_is_named_as_the_picker_names_it() {
        let names: Vec<&str> = [
            EngineKind::LlamaServer,
            EngineKind::Vllm,
            EngineKind::SpeechHost,
            EngineKind::KokoroFastApi,
            EngineKind::Strata,
        ]
        .into_iter()
        .map(|kind| Engine::of(kind).name())
        .collect();
        assert_eq!(
            names,
            ["llama.cpp", "vLLM", "the speech engine", "Kokoro", "Strata"]
        );
    }

    #[test]
    fn strata_serves_what_the_other_model_engines_serve_and_is_found_by_its_folder() {
        assert!(Engine::Strata.serves(Slot::Text) && !Engine::Strata.serves(Slot::VoiceIn));
        assert_eq!(
            Engine::Strata.state(&EngineConfig::default()),
            EngineState::Missing
        );
        let elsewhere = EngineConfig {
            strata_folder: Some(PathBuf::from("/nonexistent/strata")),
            ..EngineConfig::default()
        };
        assert_eq!(Engine::Strata.state(&elsewhere), EngineState::Missing);
    }
}
