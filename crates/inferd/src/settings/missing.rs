//! The engine programs this computer lacks, as the picker says them: a person who sees no model
//! on this computer is told which program would run one, in plain words and never a path.

use porter_infer::Slot;
use porter_router::local::EngineConfig;
use std::path::Path;

/// A program that runs models on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// `llama-server`.
    LlamaCpp,
    /// vLLM's Python.
    Vllm,
    /// quire's speech host.
    Speech,
    /// Kokoro's Python.
    Kokoro,
}

impl Engine {
    const ALL: [Engine; 4] = [Self::LlamaCpp, Self::Vllm, Self::Speech, Self::Kokoro];

    /// What the person calls it.
    fn name(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama.cpp",
            Self::Vllm => "vLLM",
            Self::Speech => "the speech engine",
            Self::Kokoro => "Kokoro",
        }
    }

    /// Whether a model of `slot` can run on it.
    fn serves(self, slot: Slot) -> bool {
        let speech = matches!(slot, Slot::VoiceIn | Slot::VoiceOut);
        match self {
            Self::LlamaCpp | Self::Vllm => !speech,
            Self::Speech => speech,
            Self::Kokoro => slot == Slot::VoiceOut,
        }
    }

    fn program(self, config: &EngineConfig) -> Option<&Path> {
        match self {
            Self::LlamaCpp => config.llama_server.as_deref(),
            Self::Vllm => config.vllm_python.as_deref(),
            Self::Speech => config.speech_host.as_deref(),
            Self::Kokoro => config.kokoro_python.as_deref(),
        }
    }
}

/// The engines with no program on this computer: not named in the file, or named and not there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingEngines(Vec<Engine>);

impl MissingEngines {
    /// The engines `config` has no installed program for.
    pub fn of(config: &EngineConfig) -> Self {
        Self(
            Engine::ALL
                .into_iter()
                .filter(|engine| !engine.program(config).is_some_and(Path::exists))
                .collect(),
        )
    }

    /// The sentence for a slot with no model on this computer, when a missing program is why.
    pub fn note(&self, slot: Slot) -> Option<String> {
        let names: Vec<&str> = self
            .0
            .iter()
            .filter(|engine| engine.serves(slot))
            .map(|engine| engine.name())
            .collect();
        match names.as_slice() {
            [] => None,
            [one] => Some(format!(
                "Running models on this computer needs {one}, which is not installed."
            )),
            many => Some(format!(
                "Running models on this computer needs {}, which are not installed.",
                many.join(" or ")
            )),
        }
    }
}
