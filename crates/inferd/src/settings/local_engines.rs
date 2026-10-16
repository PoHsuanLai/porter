//! The engines inferd knows and whether each has a program here, as `Inference1.LocalEngines`
//! tells a client. It reads the same resolved [`EngineConfig`] the engine starter and
//! [`super::MissingEngines`] read, so the three agree.

use super::missing::Engine;
use porter_dbus::LocalEngine;
use porter_infer::Slot;
use porter_router::local::EngineConfig;

/// One entry per engine kind, in the picker's order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalEngines(Vec<LocalEngine>);

impl LocalEngines {
    /// The engines of `config`: found when a program is named and there, else missing.
    pub fn of(config: &EngineConfig) -> Self {
        Self(
            Engine::ALL
                .into_iter()
                .map(|engine| LocalEngine {
                    id: engine.slug().to_owned(),
                    name: engine.name().to_owned(),
                    state: engine.state(config),
                    slots: Slot::ALL
                        .into_iter()
                        .filter(|slot| engine.serves(*slot))
                        .map(|slot| slot.slug().to_owned())
                        .collect(),
                })
                .collect(),
        )
    }

    /// The entries.
    pub fn rows(&self) -> &[LocalEngine] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_dbus::EngineState;
    use std::path::PathBuf;

    #[test]
    fn a_named_program_that_is_there_is_found_and_the_rest_are_missing() {
        let here = std::env::current_exe().expect("this test's own executable");
        let config = EngineConfig {
            llama_server: Some(here.clone()),
            vllm_python: Some(PathBuf::from("/nonexistent/python")),
            ..EngineConfig::default()
        };
        let engines = LocalEngines::of(&config);
        let states: Vec<(&str, &EngineState)> = engines
            .rows()
            .iter()
            .map(|row| (row.id.as_str(), &row.state))
            .collect();
        assert_eq!(
            states,
            [
                ("llama_cpp", &EngineState::Found(here)),
                ("vllm", &EngineState::Missing),
                ("speech", &EngineState::Missing),
                ("kokoro", &EngineState::Missing),
            ]
        );
        let slots = |index: usize| engines.rows()[index].slots.clone();
        assert!(
            slots(0).contains(&"text".to_owned()) && !slots(0).contains(&"voice_out".to_owned())
        );
        assert_eq!(slots(3), ["voice_out"]);
    }
}
