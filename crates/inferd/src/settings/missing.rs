//! The engine programs this computer can run models with, by the names the picker and the bus
//! give them: which slots each serves and whether its program is there.

use engine_spec::engine;
use model_catalog::EngineKind;
use porter_dbus::inference::EngineState;
use porter_infer::Slot;
use porter_router::local::EngineConfig;

/// What the person calls the engine of `kind` in the picker (copy text, so it is not the engine
/// spec's: the spec holds no words a person reads).
pub(crate) fn engine_name(kind: EngineKind) -> &'static str {
    match kind {
        EngineKind::LlamaServer => "llama.cpp",
        EngineKind::Vllm => "vLLM",
        EngineKind::Strata => "Strata",
        EngineKind::SpeechHost => "the speech engine",
        EngineKind::KokoroFastApi => "Kokoro",
    }
}

/// Whether a model of `slot` can run on the engine of `kind`: speech models only on a speech
/// engine of their direction, every other slot on an engine that is not one.
pub(super) fn serves(kind: EngineKind, slot: Slot) -> bool {
    let spec = engine(kind);
    match slot {
        Slot::VoiceIn => spec.hears(),
        Slot::VoiceOut => spec.speaks(),
        _ => !spec.hears() && !spec.speaks(),
    }
}

/// Whether the program of the engine of `kind` is there: named in `config` (the programs already
/// resolved by [`crate::config::ProgramSearch`]) and present. Strata's is the Python of the folder
/// the person pointed at.
pub(super) fn state(kind: EngineKind, config: &EngineConfig) -> EngineState {
    match config.program(kind).filter(|path| path.exists()) {
        Some(path) => EngineState::Found(path),
        None => EngineState::Missing,
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
        .map(engine_name)
        .collect();
        assert_eq!(
            names,
            ["llama.cpp", "vLLM", "the speech engine", "Kokoro", "Strata"]
        );
    }

    #[test]
    fn strata_serves_what_the_other_model_engines_serve_and_is_found_by_its_folder() {
        assert!(
            serves(EngineKind::Strata, Slot::Text) && !serves(EngineKind::Strata, Slot::VoiceIn)
        );
        assert_eq!(
            state(EngineKind::Strata, &EngineConfig::default()),
            EngineState::Missing
        );
        let elsewhere = EngineConfig {
            strata_folder: Some(std::path::PathBuf::from("/nonexistent/strata")),
            ..EngineConfig::default()
        };
        assert_eq!(state(EngineKind::Strata, &elsewhere), EngineState::Missing);
    }
}
