//! The engines inferd knows and where each stands, as `Inference1.LocalEngines` tells a client.
//! The program of each is read from the same resolved [`EngineConfig`] the engine starter reads,
//! so the two agree; an engine that can be installed also says what an install would be, and one
//! being installed says how far it has come.

use super::missing::Engine;
use crate::install::{EngineInstalls, Standing};
use porter_dbus::{EnginePlace, EngineState, LocalEngine};
use porter_engines::{EngineId, Place};
use porter_infer::Slot;
use porter_router::local::EngineConfig;

/// The engines of one daemon.
#[derive(Debug, Clone, Default)]
pub struct LocalEngines {
    config: EngineConfig,
    installs: Option<EngineInstalls>,
}

impl LocalEngines {
    /// The engines of `config`: found when a program is named and there, else missing.
    pub fn of(config: &EngineConfig) -> Self {
        Self {
            config: config.clone(),
            installs: None,
        }
    }

    /// The same, saying also what could be installed, what is being installed and what failed.
    #[must_use]
    pub fn with_installs(self, installs: EngineInstalls) -> Self {
        Self {
            installs: Some(installs),
            ..self
        }
    }

    /// One row per engine kind, in the picker's order.
    pub fn rows(&self) -> Vec<LocalEngine> {
        Engine::ALL
            .into_iter()
            .map(|engine| self.row(engine))
            .collect()
    }

    fn row(&self, engine: Engine) -> LocalEngine {
        let mut row = LocalEngine {
            id: engine.slug().to_owned(),
            name: engine.name().to_owned(),
            state: engine.state(&self.config),
            slots: Slot::ALL
                .into_iter()
                .filter(|slot| engine.serves(*slot))
                .map(|slot| slot.slug().to_owned())
                .collect(),
            version: None,
            place: None,
            offer: None,
        };
        let (Some(installs), Some(id)) = (&self.installs, installable(engine)) else {
            return row;
        };
        match installs.standing(id) {
            Standing::Ready(found) => {
                row.state = EngineState::Found(found.program);
                row.version = found.version;
                row.place = Some(place_of(found.place));
            }
            Standing::Installing(thousandths) => row.state = EngineState::Installing(thousandths),
            Standing::Failed(why) => {
                row.state = EngineState::Failed(why.sentence().to_owned());
                row.offer = installs.offer(id);
            }
            Standing::Changed => {
                row.state = EngineState::Failed(format!(
                    "{} was changed since it was installed. Reinstall it?",
                    engine.name()
                ));
                row.offer = installs.offer(id);
            }
            Standing::NotInstalled => row.offer = installs.offer(id),
        }
        row
    }
}

/// The engine the installs know by this name, when they do.
fn installable(engine: Engine) -> Option<EngineId> {
    match engine {
        Engine::LlamaCpp => Some(EngineId::LlamaCpp),
        Engine::Vllm => Some(EngineId::Vllm),
        Engine::Strata => Some(EngineId::Strata),
        Engine::Speech | Engine::Kokoro => None,
    }
}

fn place_of(place: Place) -> EnginePlace {
    match place {
        Place::Package => EnginePlace::Package,
        Place::System => EnginePlace::System,
        Place::Person => EnginePlace::Person,
        Place::Pointed => EnginePlace::Pointed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_named_program_that_is_there_is_found_and_the_rest_are_missing() {
        let here = std::env::current_exe().expect("this test's own executable");
        let config = EngineConfig {
            llama_server: Some(here.clone()),
            vllm_python: Some(PathBuf::from("/nonexistent/python")),
            ..EngineConfig::default()
        };
        let engines = LocalEngines::of(&config).rows();
        let states: Vec<(&str, &EngineState)> = engines
            .iter()
            .map(|row| (row.id.as_str(), &row.state))
            .collect();
        assert_eq!(
            states,
            [
                ("llama_cpp", &EngineState::Found(here)),
                ("vllm", &EngineState::Missing),
                ("strata", &EngineState::Missing),
                ("speech", &EngineState::Missing),
                ("kokoro", &EngineState::Missing),
            ]
        );
        let slots = |index: usize| engines[index].slots.clone();
        assert!(
            slots(0).contains(&"text".to_owned()) && !slots(0).contains(&"voice_out".to_owned())
        );
        assert_eq!(slots(4), ["voice_out"]);
    }
}
