//! What loading a model now would take, from stoker's pure `budget`: the router's input
//! `SwapCost`. Nothing here starts or stops anything; it asks the question "if I wanted this
//! engine now, whom would the supervisor unload?" and says which of those answers is a swap Automatic
//! may make. `budget` never names an engine that is in a turn, and the answer says what each
//! victim is doing so the router can check that again.

use engine_supervisor::{
    BudgetVerdict, EngineId, EngineSpec, EngineState, GpuMemory, MonoMs, budget,
};
use model_catalog::MiB;
use porter_infer::{EngineLoad, ModelRef, SwapCost};
use std::time::Duration;

/// The engines that hold memory now: their id, what they hold and when they were last used.
pub type Running = Vec<(EngineId, MiB, MonoMs)>;

/// What the memory budget looks like now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// The GPU as last observed; `None` before the supervisor has looked once.
    pub gpu: Option<GpuMemory>,
    /// Memory kept free beside an engine.
    pub headroom: MiB,
    /// The supervisor's clock.
    pub now: MonoMs,
    /// How recently an engine must have been used to count as in a turn.
    pub probe_every: Duration,
}

impl Budget {
    /// The budget at supervisor time `now`: `gpu` as last observed, `headroom` kept free beside
    /// an engine, and an engine used within `probe_every` counting as in a turn.
    pub fn new(gpu: Option<GpuMemory>, headroom: MiB, now: MonoMs, probe_every: Duration) -> Self {
        Self {
            gpu,
            headroom,
            now,
            probe_every,
        }
    }
}

/// The engines of `states` that are ready, with the memory `spec_of` says each holds.
pub fn running_of<'a>(
    states: impl IntoIterator<Item = (&'a EngineId, &'a EngineState)>,
    need_of: impl Fn(&EngineId) -> Option<MiB>,
) -> Running {
    states
        .into_iter()
        .filter_map(|(id, state)| match (state, need_of(id)) {
            (EngineState::Ready { last_used, .. }, Some(need)) => {
                Some((id.clone(), need, *last_used))
            }
            _ => None,
        })
        .collect()
}

/// The cost of loading `want` now. `model_of` names an engine's model for a victim;
/// `cold_start_estimate_s` is the catalogue's estimate. Before the GPU has been observed nothing
/// is known to be in the way, so the answer is `Fits` (the supervisor decides again when asked
/// to start the engine).
pub fn swap_cost(
    want: &EngineSpec,
    running: &Running,
    budget_now: Budget,
    model_of: impl Fn(&EngineId) -> Option<ModelRef>,
    cold_start_estimate_s: u16,
) -> SwapCost {
    let Some(gpu) = budget_now.gpu else {
        return SwapCost::Fits {
            cold_start_estimate_s,
        };
    };
    let verdict = budget(
        want,
        running,
        gpu,
        budget_now.headroom,
        budget_now.now,
        budget_now.probe_every,
    );
    match verdict {
        BudgetVerdict::Fits => SwapCost::Fits {
            cold_start_estimate_s,
        },
        BudgetVerdict::NoRoom { .. } => SwapCost::NoRoom,
        BudgetVerdict::EvictFirst(victims) => match victims.as_slice() {
            [one] => match (model_of(one), load_of(one, running, budget_now)) {
                (Some(victim), load) => SwapCost::Evicts {
                    victim,
                    load,
                    cold_start_estimate_s,
                },
                // An engine that is not one of our models cannot be named to the person.
                (None, _) => SwapCost::NoRoom,
            },
            many => SwapCost::Purge {
                engines: u8::try_from(many.len()).unwrap_or(u8::MAX),
            },
        },
    }
}

fn load_of(id: &EngineId, running: &Running, budget_now: Budget) -> EngineLoad {
    let window = u64::try_from(budget_now.probe_every.as_millis()).unwrap_or(u64::MAX);
    match running.iter().find(|(one, _, _)| one == id) {
        Some((_, _, last)) if budget_now.now.0.saturating_sub(last.0) < window => {
            EngineLoad::MidTurn
        }
        _ => EngineLoad::Idle,
    }
}

#[cfg(test)]
mod tests;
