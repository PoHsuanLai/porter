//! Who a route is for: whose Automatic pool it reads, and which models other helpers are using.

use super::Engines;
use crate::router::Listed;
use crate::router::placed::Placed;
use crate::seats::Gate;
use porter_infer::{Asker, Chooser, ModelRef, Sharing};

/// The asker of a route, as routing needs it. The default is the person's own turn with nothing
/// shared, which is what a route without a session (Availability, Prepare) means.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Who {
    /// Whose pool Automatic chooses from.
    pub chooser: Chooser,
    /// The models a helper other than the asker is using now.
    pub shared: Vec<ModelRef>,
}

impl Who {
    /// `asker` as the gate sees the machine now: a model is shared when another helper has a
    /// request on an engine instance running it.
    pub fn of(asker: &Asker, gate: &Gate, engines: &Engines) -> Self {
        Self {
            chooser: asker.chooser(),
            shared: engines.models_of_other_helpers(gate, asker),
        }
    }

    /// `listed` with each model another helper uses marked as shared.
    pub fn mark(&self, listed: Vec<Listed>) -> Vec<Listed> {
        listed.into_iter().map(|one| self.mark_one(one)).collect()
    }

    /// `placed` with each model another helper uses marked as shared.
    pub fn mark_placed(&self, placed: Vec<Placed>) -> Vec<Placed> {
        placed
            .into_iter()
            .map(|one| Placed {
                listed: self.mark_one(one.listed),
                ..one
            })
            .collect()
    }

    fn mark_one(&self, listed: Listed) -> Listed {
        let model = ModelRef {
            account: listed.card.account.clone(),
            model: listed.card.model.clone(),
        };
        match self.shared.contains(&model) {
            true => Listed {
                sharing: Sharing::HeldByHelper,
                ..listed
            },
            false => listed,
        }
    }
}

impl Engines {
    /// The local models a helper other than `asker` has a request on now.
    fn models_of_other_helpers(&self, gate: &Gate, asker: &Asker) -> Vec<ModelRef> {
        gate.engines_of_other_helpers(asker)
            .iter()
            .filter_map(|id| {
                self.book
                    .local
                    .iter()
                    .find(|model| model.spec.id == *id)
                    .map(|model| model.model_ref())
            })
            .collect()
    }
}
