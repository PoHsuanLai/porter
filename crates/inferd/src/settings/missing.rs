//! The engine programs this computer lacks, as the picker says them: a person who sees no model
//! on this computer is told which program would run one, in plain words and never a path.

use model_catalog::EngineKind;
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

    /// The engine a catalogue profile of `kind` starts.
    pub(super) fn of(kind: EngineKind) -> Self {
        match kind {
            EngineKind::LlamaServer => Self::LlamaCpp,
            EngineKind::Vllm => Self::Vllm,
            EngineKind::SpeechHost => Self::Speech,
            EngineKind::KokoroFastApi => Self::Kokoro,
        }
    }

    /// What the person calls it.
    pub(super) fn name(self) -> &'static str {
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

    /// The sentence for a slot with no model on this computer, when a missing program is why:
    /// every engine that could run the slot is missing (one that is there means it is not).
    pub fn note(&self, slot: Slot) -> Option<String> {
        let serving: Vec<Engine> = Engine::ALL
            .into_iter()
            .filter(|engine| engine.serves(slot))
            .collect();
        if !serving.iter().all(|engine| self.0.contains(engine)) {
            return None;
        }
        let names: Vec<&str> = serving.iter().map(|engine| engine.name()).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_is_noted_only_when_every_engine_that_could_run_it_is_missing() {
        let nothing = MissingEngines(Engine::ALL.to_vec());
        assert_eq!(
            nothing.note(Slot::Text).as_deref(),
            Some(
                "Running models on this computer needs llama.cpp or vLLM, which are not installed."
            )
        );
        assert_eq!(
            nothing.note(Slot::VoiceIn).as_deref(),
            Some(
                "Running models on this computer needs the speech engine, which is not installed."
            )
        );
        let llama_here = MissingEngines(vec![Engine::Vllm, Engine::Speech, Engine::Kokoro]);
        assert_eq!(llama_here.note(Slot::Text), None);
        assert_eq!(MissingEngines::default().note(Slot::VoiceOut), None);
    }

    #[test]
    fn a_catalogue_engine_kind_is_named_as_the_notes_name_it() {
        let names: Vec<&str> = [
            EngineKind::LlamaServer,
            EngineKind::Vllm,
            EngineKind::SpeechHost,
            EngineKind::KokoroFastApi,
        ]
        .into_iter()
        .map(|kind| Engine::of(kind).name())
        .collect();
        assert_eq!(names, ["llama.cpp", "vLLM", "the speech engine", "Kokoro"]);
    }
}
