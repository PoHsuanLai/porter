//! The window an engine is started with. The supervisor's machine starts every engine with the
//! unit its spec holds, which asks for the window the catalogue serves. For a model whose engine
//! takes its window as a launch flag, the driver asks [`Sizing`] for the unit to start with
//! instead: the window the person set (`ai.length.conversation`), or else the largest one the
//! card has memory for, as `porter_router::local::choose_window` works it out.
//!
//! The window chosen is remembered while the engine runs. A reply is bounded by it, and the
//! memory of the engine is counted at it when another engine's window is chosen. A change of
//! `ai.length.conversation` is read the next time an engine starts.

use super::Programs;
use crate::local::{EngineConfig, Lengths, LocalModel};
use engine_supervisor::{EngineId, UnitSpec, Window};
use model_catalog::MiB;
use model_provider::Tokens;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Chooses the window of the engines it knows. Cloning shares it.
#[derive(Debug, Clone, Default)]
pub struct Sizing {
    shared: Arc<Shared>,
    /// The programs engines are started with now (the units say the ones from start).
    programs: Programs,
}

#[derive(Debug, Default)]
struct Shared {
    /// The models whose window is chosen at start.
    plans: BTreeMap<EngineId, LocalModel>,
    /// The programs and cache the units are made with.
    engines: EngineConfig,
    /// What the person set, replaced when the settings are put in force.
    lengths: Mutex<Lengths>,
    /// The window each engine was last started with.
    chosen: Mutex<BTreeMap<EngineId, Tokens>>,
}

fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Sizing {
    /// Sizing for the models among `models` whose window is chosen at start, made with
    /// `engines` (the same configuration their units were first made with), under `lengths`.
    pub fn for_models(engines: EngineConfig, models: &[LocalModel], lengths: Lengths) -> Self {
        let plans = models
            .iter()
            .filter(|model| model.sizable())
            .map(|model| (model.spec.id.clone(), model.clone()))
            .collect();
        Self {
            shared: Arc::new(Shared {
                plans,
                engines,
                lengths: Mutex::new(lengths),
                chosen: Mutex::default(),
            }),
            programs: Programs::default(),
        }
    }

    /// The same, starting each engine with the program `programs` has now.
    #[must_use]
    pub fn with_programs(self, programs: Programs) -> Self {
        Self { programs, ..self }
    }

    /// `unit` with the program now in force (see [`Programs`]).
    pub(super) fn program_in(&self, unit: UnitSpec) -> UnitSpec {
        self.programs.apply(unit)
    }

    /// What the person set now; read the next time an engine starts.
    pub fn set_lengths(&self, lengths: Lengths) {
        *held(&self.shared.lengths) = lengths;
    }

    /// What the person set now.
    pub fn lengths(&self) -> Lengths {
        *held(&self.shared.lengths)
    }

    /// The window the engine was last started with; none for an engine whose window is not
    /// chosen, or one not started yet.
    pub fn window_of(&self, id: &EngineId) -> Option<Tokens> {
        held(&self.shared.chosen).get(id).copied()
    }

    /// The memory the engine takes: `served` (its spec's) unless it was started with a window of
    /// its own.
    pub(super) fn need_of(&self, id: &EngineId, served: MiB) -> MiB {
        match (self.shared.plans.get(id), self.window_of(id)) {
            (Some(model), Some(window)) => model.need_with(Some(window)),
            _ => served,
        }
    }

    /// The unit to start `id` with when `free` of a card of `total` is left for it, and the
    /// window it has (remembered). None for an engine whose window is not chosen: it starts as
    /// its spec says.
    pub(super) fn unit(
        &self,
        id: &EngineId,
        free: Option<MiB>,
        total: Option<MiB>,
    ) -> Option<UnitSpec> {
        let model = self.shared.plans.get(id)?;
        let window = model.window(self.lengths(), free, total)?;
        held(&self.shared.chosen).insert(id.clone(), window);
        Some(model.unit_with(&self.shared.engines, Window::Chosen(window)))
    }
}
