//! The engine host: stoker's `engine-supervisor` machine driven over an `EngineHost` (child
//! processes today, systemd transient units later), with the GPU as a shared resource, and the
//! book of models a session may be routed to. `Engines` is the one object the router, the
//! `Inference1` handler and the session server share: it knows the models, their readiness, and
//! how to ask for an engine.

use crate::local::{LocalModel, Weights};
use crate::peers::Role;
use crate::router::{Listed, choose};
use crate::runner::{Pin, Pinned};
use crate::serve::{EngineFailed, EngineHost};
use crate::session::{RouteDecision, SessionSpec};
use crate::supervise::{Snapshot, Supervised};
use crate::swap::{Budget, running_of, swap_cost};
use engine_supervisor::{EngineId, EngineState, MonoMs};
use model_catalog::Licence;
use porter_core::consent::Availability;
use porter_core::{DataClass, Need, Tier};
use porter_infer::{
    AutoPolicy, InferRefusal, LicenceClass, ModelCard, ModelRef, PickRefusal, Policy, Readiness,
    ServedBy, SwapCost, TierMap,
};
use std::sync::Arc;
use std::time::Duration;

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

/// What routing and readiness are computed from.
#[derive(Debug, Default)]
struct Book {
    local: Vec<Arc<LocalModel>>,
    remote: Vec<ModelCard>,
    policy: Option<Policy>,
    tiers: TierMap,
    auto: AutoPolicy,
}

/// Every engine inferd supervises, and the models behind them.
#[derive(Debug, Clone)]
pub struct Engines {
    book: Arc<Book>,
    supervised: Supervised,
}

impl Default for Engines {
    /// No models and no engines: every route is `Unavailable`.
    fn default() -> Self {
        Self {
            book: Arc::default(),
            supervised: Supervised::idle(),
        }
    }
}

impl Engines {
    /// The local models, the supervisor that runs their engines, the user's policy and tier map.
    pub fn new(
        local: Vec<LocalModel>,
        supervised: Supervised,
        policy: Policy,
        tiers: TierMap,
    ) -> Self {
        Self {
            book: Arc::new(Book {
                local: local.into_iter().map(Arc::new).collect(),
                remote: Vec::new(),
                policy: Some(policy),
                tiers,
                auto: AutoPolicy::default(),
            }),
            supervised,
        }
    }

    /// The same, with the `ai.auto.*` rows the daemon read (the default rows otherwise).
    pub fn with_auto(self, auto: AutoPolicy) -> Self {
        let book = Book {
            local: self.book.local.clone(),
            remote: self.book.remote.clone(),
            policy: self.book.policy.clone(),
            tiers: self.book.tiers.clone(),
            auto,
        };
        Self {
            book: Arc::new(book),
            supervised: self.supervised,
        }
    }

    /// The same, with models of accounts that are not on this computer. They take part in
    /// routing (so a floor refuses them), but nothing serves a turn from them yet: a route that
    /// lands on one is `Unavailable`.
    pub fn with_remote(self, remote: Vec<ModelCard>) -> Self {
        let book = Book {
            local: self.book.local.clone(),
            remote,
            policy: self.book.policy.clone(),
            tiers: self.book.tiers.clone(),
            auto: self.book.auto,
        };
        Self {
            book: Arc::new(book),
            supervised: self.supervised,
        }
    }

    /// The supervisor handle (the runner marks engines used through it).
    pub fn supervised(&self) -> &Supervised {
        &self.supervised
    }

    fn policy(&self) -> Policy {
        self.book.policy.clone().unwrap_or_else(Policy::proposed)
    }

    fn local(&self, model: &ModelRef) -> Option<&Arc<LocalModel>> {
        self.book.local.iter().find(|one| one.model_ref() == *model)
    }

    /// How ready one local model is now.
    pub fn readiness(&self, model: &LocalModel) -> Readiness {
        readiness_in(&self.supervised.snapshot(), model)
    }

    /// Every model a session may be routed to, with its readiness now and what loading it would
    /// take (stoker's `budget`, asked speculatively: nothing is started).
    pub fn listed(&self) -> Vec<Listed> {
        let snapshot = self.supervised.snapshot();
        let running = running_of(snapshot.states.iter(), |id| {
            self.book
                .local
                .iter()
                .find(|model| model.spec.id == *id)
                .map(|model| model.spec.need)
        });
        let budget_now = Budget {
            gpu: snapshot.gpu,
            headroom: self.supervised.headroom(),
            now: snapshot.now.unwrap_or(MonoMs(0)),
            probe_every: self.supervised.probe_every(),
        };
        let model_of = |id: &EngineId| {
            self.book
                .local
                .iter()
                .find(|model| model.spec.id == *id)
                .map(|model| model.model_ref())
        };
        let local = self.book.local.iter().map(|model| {
            let readiness = readiness_in(&snapshot, model);
            let swap = match readiness {
                Readiness::Ready | Readiness::Loading => SwapCost::Resident,
                _ => swap_cost(
                    &model.spec,
                    &running,
                    budget_now,
                    model_of,
                    model.entry.cold_start_estimate_s.0,
                ),
            };
            Listed {
                card: model.card.clone(),
                readiness,
                swap,
                licence: licence_of(&model.entry.licence),
            }
        });
        let remote = self.book.remote.iter().map(|card| Listed {
            card: card.clone(),
            readiness: Readiness::Ready,
            swap: SwapCost::Resident,
            licence: LicenceClass::Proprietary,
        });
        local.chain(remote).collect()
    }

    /// Decides who answers a session, and what the runner is to be pinned to. A need no runner
    /// can serve yet (speech) and a model that is not on this computer are `Unavailable`.
    pub fn route(
        &self,
        spec: &SessionSpec,
        role: Role,
    ) -> Result<(RouteDecision, Pinned), InferRefusal> {
        self.route_detailed(spec, role).map_err(|r| r.refusal)
    }

    /// `route`, with the reason a named model could not serve when that is why it refused.
    pub fn route_detailed(
        &self,
        spec: &SessionSpec,
        role: Role,
    ) -> Result<(RouteDecision, Pinned), PickRefusal> {
        if matches!(spec.need, Need::ComputerUse(_)) && role != Role::Cua {
            return Err(InferRefusal::Denied.into());
        }
        let decided = choose(
            &spec.need,
            spec.class,
            spec.tier,
            &self.listed_for(&spec.need),
            &self.policy(),
            &self.book.tiers,
            self.book.auto,
        )?;
        let (chosen, readiness) = (decided.chosen, decided.readiness);
        let model = self
            .local(&ModelRef {
                account: chosen.account.clone(),
                model: chosen.model.clone(),
            })
            .cloned()
            .ok_or(PickRefusal::from(InferRefusal::Unavailable))?;
        let served = ServedBy {
            account: chosen.account,
            model: chosen.model,
            locality: model.card.locality.clone(),
        };
        Ok((
            RouteDecision {
                served: served.clone(),
                readiness,
                why: decided.why,
                show: self.book.auto.show_reason,
            },
            Pinned {
                served,
                model: Some(model),
            },
        ))
    }

    /// Models for a need; none when no runner serves that kind of need yet.
    fn listed_for(&self, need: &Need) -> Vec<Listed> {
        match need {
            Need::Speech(_) => Vec::new(),
            _ => self.listed(),
        }
    }

    /// Warms the engine the route would pick for these arguments and answers its readiness
    /// (`Inference1.Prepare`). Speech-to-text engines are CPU-only and cost no VRAM. A stopped
    /// engine is asked for and answers `Loading`; the wait is the caller's next `Open`.
    pub async fn prepare(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
    ) -> Result<Readiness, InferRefusal> {
        self.prepare_as(need, class, tier, Role::App).await
    }

    /// `prepare` for a caller of this role (only cuad may warm a computer-use model).
    pub async fn prepare_as(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        role: Role,
    ) -> Result<Readiness, InferRefusal> {
        let spec = SessionSpec {
            need: need.clone(),
            class,
            tier,
        };
        let (decision, pinned) = self.route(&spec, role)?;
        match (decision.readiness, pinned.model) {
            (Readiness::Loadable, Some(model)) => {
                self.supervised.warm(&model.spec.id);
                Ok(Readiness::Loading)
            }
            (readiness, _) => Ok(readiness),
        }
    }

    /// The GPU's state now: loading if any supervised engine is starting, busy if a turn is
    /// using one, else idle.
    pub fn gpu(&self) -> GpuState {
        gpu_in(&self.supervised.snapshot(), self.supervised.probe_every())
    }

    /// What an app is told about a need without a session (`Inference1.Availability`): whether
    /// the route would run, without revealing which account or model.
    pub fn availability(&self, need: &Need, class: DataClass, role: Role) -> Availability {
        let spec = SessionSpec {
            need: need.clone(),
            class,
            tier: Tier::Balanced,
        };
        match self.route(&spec, role) {
            Ok(_) => Availability::Granted,
            Err(InferRefusal::NeedsGrant) => Availability::AvailableNeedsConsent,
            Err(InferRefusal::Denied | InferRefusal::OverBudget) => Availability::Denied,
            Err(InferRefusal::Unsupported) => Availability::Unsupported,
            Err(InferRefusal::RequiresCloud(_) | InferRefusal::Unavailable) => {
                Availability::NeedsAccount
            }
        }
    }

    /// The router for one session of a caller of this role: it decides once and writes the
    /// decision to `pin`.
    pub fn router(&self, pin: Pin, role: Role) -> SessionRouter {
        SessionRouter {
            engines: self.clone(),
            pin,
            role,
        }
    }
}

fn licence_of(licence: &Licence) -> LicenceClass {
    match licence {
        Licence::Open(_) => LicenceClass::Open,
        Licence::NonCommercial(_) => LicenceClass::NonCommercial,
        Licence::Proprietary => LicenceClass::Proprietary,
    }
}

fn readiness_in(snapshot: &Snapshot, model: &LocalModel) -> Readiness {
    if model.weights() == Weights::Missing {
        return Readiness::Downloadable;
    }
    match snapshot.states.get(&model.spec.id) {
        Some(EngineState::Ready { .. }) => Readiness::Ready,
        Some(EngineState::Starting { .. } | EngineState::Backoff { .. }) => Readiness::Loading,
        Some(EngineState::Stopped | EngineState::Stopping { .. }) => Readiness::Loadable,
        Some(EngineState::Failed(_)) | None => Readiness::Unavailable,
    }
}

fn gpu_in(snapshot: &Snapshot, in_turn: Duration) -> GpuState {
    let starting = snapshot
        .states
        .values()
        .any(|state| matches!(state, EngineState::Starting { .. }));
    let window = u64::try_from(in_turn.as_millis()).unwrap_or(u64::MAX);
    let busy = |state: &EngineState| match (state, snapshot.now) {
        (EngineState::Ready { last_used, .. }, Some(now)) => {
            now.0.saturating_sub(last_used.0) < window
        }
        _ => false,
    };
    match (starting, snapshot.states.values().any(busy)) {
        (true, _) => GpuState::Loading,
        (false, true) => GpuState::Busy,
        (false, false) => GpuState::Idle,
    }
}

impl EngineHost for Engines {
    async fn want(&self, model: ModelRef) -> Result<(), EngineFailed> {
        let local = self.local(&model).ok_or(EngineFailed)?;
        self.supervised
            .want(&local.spec.id)
            .await
            .map_err(|_| EngineFailed)
    }

    /// Engines unload when idle (the supervisor's timer), not when a session ends: the next
    /// session would pay the cold start again.
    fn release(&self, _model: &ModelRef) {}
}

/// The route of one session: decides through the engines, remembers what it decided for the
/// runner.
#[derive(Debug, Clone)]
pub struct SessionRouter {
    engines: Engines,
    pin: Pin,
    role: Role,
}

impl crate::serve::Router for SessionRouter {
    async fn route(&self, spec: &SessionSpec) -> Result<RouteDecision, PickRefusal> {
        let (decision, pinned) = self.engines.route_detailed(spec, self.role)?;
        self.pin.set(pinned);
        Ok(decision)
    }
}

#[cfg(test)]
mod tests;
