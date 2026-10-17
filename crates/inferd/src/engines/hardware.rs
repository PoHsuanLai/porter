//! What this machine can say about one more request on a model, from what inferd already
//! watches: the supervisor's reading of the graphics card, the engines holding memory now, the
//! catalogue's size of the model, and the gate's count of helpers and waiting requests. The
//! notes themselves are decided by `porter_infer::hardware_notes`.

use super::{Engines, Offered};
use crate::local::LocalModel;
use crate::peers::Caller;
use crate::router::Unchosen;
use crate::router::placed::Allowed;
use crate::seats::Gate;
use crate::session::SessionSpec;
use crate::swap::running_of;
use model_catalog::{GpuNeed, MiB};
use porter_core::Bytes;
use porter_infer::{HardwareFacts, HardwareNote, MemoryFacts, ModelRef, hardware_notes};

fn bytes_of(mib: MiB) -> Bytes {
    Bytes(u64::from(mib.0) << 20)
}

impl Engines {
    /// The graphics memory that is free now, when the card has been read: what is left after
    /// others' use and the engines this daemon holds.
    fn free_graphics_memory(&self) -> Option<MiB> {
        let snapshot = self.supervised.snapshot();
        let gpu = snapshot.gpu?;
        let held: u32 = running_of(snapshot.states.iter(), |id| {
            self.book
                .local
                .iter()
                .find(|model| model.spec.id == *id)
                .map(|model| model.spec.need)
        })
        .iter()
        .map(|(_, need, _)| need.0)
        .fold(0, u32::saturating_add);
        Some(MiB(gpu
            .total
            .0
            .saturating_sub(gpu.used_by_others.0)
            .saturating_sub(held)))
    }

    /// What `model` faces now: how many helpers use it, how many requests wait, and the graphics
    /// memory against what one more request needs (the catalogue's size of its context).
    pub(crate) fn facts_for(&self, model: &LocalModel, gate: &Gate) -> HardwareFacts {
        let on_card = model.entry.vram.gpu_need() == GpuNeed::Needed;
        let memory = self
            .free_graphics_memory()
            .filter(|_| on_card)
            .map(|free| MemoryFacts {
                free: bytes_of(free),
                one_more: bytes_of(MiB(model
                    .spec
                    .need
                    .0
                    .saturating_sub(model.entry.vram.weights.0)
                    .saturating_sub(model.entry.vram.overhead.0))),
            });
        HardwareFacts {
            helpers: gate.helpers_using(&model.spec.id),
            ahead: gate.waiting_on(&model.spec.id),
            memory,
        }
    }

    /// The notes for a session like `spec`: for the local model it would be routed to (the one
    /// `named` when the session names its own). Nothing for a model that is not run from here,
    /// or when the route is refused: a note is a fact about the machine, and there is none to
    /// give.
    pub async fn hardware_notes(
        &self,
        spec: &SessionSpec,
        caller: &Caller,
        allowed: Option<&Allowed>,
        named: Option<&ModelRef>,
        gate: &Gate,
    ) -> Vec<HardwareNote> {
        let offered: Offered = self.offer(&caller.app, spec.class, spec.usage).await;
        let routed = self.route_as(
            spec,
            caller.role,
            &offered,
            allowed,
            Unchosen::CatalogueChoice,
            named,
        );
        match routed {
            Ok((_, pinned)) => pinned
                .model
                .map(|model| hardware_notes(&self.facts_for(&model, gate)))
                .unwrap_or_default(),
            // refused, or placed nowhere: no fact to give
            Err(_) => Vec::new(),
        }
    }
}
