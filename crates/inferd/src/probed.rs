//! The models of the runtimes the person runs themselves, as the daemon knows them now: the
//! book `Engines` routes through beside the models it supervises. A runtime is `Online` while a
//! probe finds it and `Offline` after (its models stay listed, with nothing to serve them; they
//! are never forgotten for a runtime that stopped). The book is shared by every clone of the
//! engines; `changed` wakes whoever tells listeners to read again.

use crate::local::LocalModel;
use crate::probe::Runtime;
use porter_core::AccountId;
use porter_infer::{ModelRef, Readiness};
use std::sync::{Arc, PoisonError, RwLock};
use tokio::sync::Notify;

/// Whether a runtime answered the last look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It answered: its models are `Ready`.
    Online,
    /// It did not: its models are `Unavailable`.
    Offline,
}

impl Standing {
    /// How ready a model of a runtime in this standing is.
    pub fn readiness(self) -> Readiness {
        match self {
            Standing::Online => Readiness::Ready,
            Standing::Offline => Readiness::Unavailable,
        }
    }
}

/// One runtime's models and whether it is running.
#[derive(Debug, Clone)]
pub struct ProbedRuntime {
    /// Which runtime.
    pub runtime: Runtime,
    /// Whether it answered the last look.
    pub standing: Standing,
    /// Its models, with each served at the runtime's port.
    pub models: Vec<Arc<LocalModel>>,
}

/// Every probed runtime.
#[derive(Debug, Clone, Default)]
pub struct ProbedBook {
    runtimes: Arc<RwLock<Vec<ProbedRuntime>>>,
    changed: Arc<Notify>,
}

impl ProbedBook {
    fn read(&self) -> Vec<ProbedRuntime> {
        self.runtimes
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Records what the last look found of `runtime` and wakes `changed`. An `Offline` runtime
    /// given no models keeps the ones it had.
    pub fn set(&self, runtime: Runtime, standing: Standing, models: Option<Vec<LocalModel>>) {
        {
            let mut held = self
                .runtimes
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            let models = models.map(|models| models.into_iter().map(Arc::new).collect::<Vec<_>>());
            match held.iter_mut().find(|one| one.runtime == runtime) {
                Some(one) => {
                    one.standing = standing;
                    if let Some(models) = models {
                        one.models = models;
                    }
                }
                None => held.push(ProbedRuntime {
                    runtime,
                    standing,
                    models: models.unwrap_or_default(),
                }),
            }
        }
        self.changed.notify_one();
    }

    /// Every runtime seen so far.
    pub fn runtimes(&self) -> Vec<ProbedRuntime> {
        self.read()
    }

    /// Every probed model, whatever its runtime is doing, with the runtime's standing.
    pub fn models(&self) -> Vec<(Arc<LocalModel>, Standing)> {
        self.read()
            .into_iter()
            .flat_map(|one| {
                let standing = one.standing;
                one.models.into_iter().map(move |model| (model, standing))
            })
            .collect()
    }

    /// The probed model `model` names.
    pub fn find(&self, model: &ModelRef) -> Option<Arc<LocalModel>> {
        self.models()
            .into_iter()
            .map(|(one, _)| one)
            .find(|one| one.model_ref() == *model)
    }

    /// How the runtime behind `account` stands; none when `account` is not a probed runtime's.
    pub fn standing_of(&self, account: &AccountId) -> Option<Standing> {
        self.read()
            .into_iter()
            .find(|one| one.runtime.provider() == account.as_str())
            .map(|one| one.standing)
    }

    /// Woken each time `set` records a look.
    pub fn changed(&self) -> Arc<Notify> {
        Arc::clone(&self.changed)
    }
}

#[cfg(test)]
mod tests;
