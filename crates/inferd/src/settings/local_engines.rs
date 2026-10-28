//! The engines inferd knows and where each stands, as `Inference1.LocalEngines` tells a client.
//! The program of each is read from the same resolved [`EngineConfig`] the engine starter reads,
//! so the two agree; an engine of ours that was installed also says which release and where from,
//! and one whose install was changed since says so.

use super::missing::{engine_name, serves, state};
use crate::installed::{Installed, Standing};
use crate::supervise::Programs;
use engine_probe::Place;
use engine_spec::{ENGINES, engine};
use model_catalog::EngineKind;
use porter_dbus::inference::{EnginePlace, EngineState, LocalEngine};
use porter_infer::Slot;
use porter_router::local::EngineConfig;

/// The engines of one daemon.
#[derive(Debug, Clone, Default)]
pub struct LocalEngines {
    config: EngineConfig,
    installed: Option<Installed>,
    programs: Option<Programs>,
}

impl LocalEngines {
    /// The engines of `config`: found when a program is named and there, else missing.
    pub fn of(config: &EngineConfig) -> Self {
        Self {
            config: config.clone(),
            installed: None,
            programs: None,
        }
    }

    /// The same, whose programs follow `programs` (what the person names while inferd runs).
    #[must_use]
    pub fn following(self, programs: Programs) -> Self {
        Self {
            programs: Some(programs),
            ..self
        }
    }

    /// The same, saying also where each engine of ours was installed and which release, and which
    /// install was changed since.
    #[must_use]
    pub fn with_installed(self, installed: Installed) -> Self {
        Self {
            installed: Some(installed),
            ..self
        }
    }

    /// One row per engine kind, in the picker's order.
    pub fn rows(&self) -> Vec<LocalEngine> {
        let config = match &self.programs {
            Some(programs) => programs.over(&self.config),
            None => self.config.clone(),
        };
        ENGINES
            .into_iter()
            .map(|kind| self.row(kind, &config))
            .collect()
    }

    fn row(&self, kind: EngineKind, config: &EngineConfig) -> LocalEngine {
        let mut row = LocalEngine {
            id: engine(kind).bus_word().to_owned(),
            name: engine_name(kind).to_owned(),
            state: state(kind, config),
            slots: Slot::ALL
                .into_iter()
                .filter(|slot| serves(kind, *slot))
                .map(|slot| slot.slug().to_owned())
                .collect(),
            version: None,
            place: None,
        };
        let Some(installed) = &self.installed else {
            return row;
        };
        match installed.standing(kind) {
            Standing::Ready(found) => {
                row.state = EngineState::Found(found.program);
                row.version = found.version;
                row.place = Some(place_of(found.place));
            }
            Standing::Changed(found) => {
                row.state = EngineState::Changed(found.program);
                row.version = found.version;
                row.place = Some(place_of(found.place));
            }
            Standing::NotInstalled => {}
        }
        row
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
