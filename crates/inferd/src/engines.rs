//! The engine host: stoker's `engine-supervisor` machine driven over systemd transient units
//! (or child processes where there is no systemd), with the GPU as a shared resource.

use porter_core::{DataClass, Need, Tier};
use porter_infer::{InferRefusal, Readiness};

/// What the GPU is doing (the `Gpu` property of `Inference1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuState {
    /// Nothing is loading or generating.
    Idle,
    /// A model is generating.
    Busy,
    /// A model is loading.
    Loading,
}

impl GpuState {
    /// The slug on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            GpuState::Idle => "idle",
            GpuState::Busy => "busy",
            GpuState::Loading => "loading",
        }
    }
}

/// Every engine inferd supervises.
#[derive(Debug, Default)]
pub struct Engines {
    _private: (),
}

impl Engines {
    /// Warms the engine the route would pick for these arguments and answers its readiness
    /// (`Inference1.Prepare`). Speech-to-text engines are CPU-only and cost no VRAM.
    pub async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> Result<Readiness, InferRefusal> {
        let _ = (need, class, tier);
        todo!("route, then Want(engine) on the supervisor, answer the engine's readiness")
    }

    /// The GPU's state now.
    pub fn gpu(&self) -> GpuState {
        todo!("loading if any supervised engine is starting, busy if a turn runs, else idle")
    }
}
