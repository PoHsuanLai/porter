//! The engine host: stoker's `engine-supervisor` machine driven over an `EngineHost` (child
//! processes today, systemd transient units later), with the GPU as a shared resource, and the
//! book of models a session may be routed to. `Engines` is the one object the router, the
//! `Inference1` handler and the session server share: it knows the models, their readiness, and
//! how to ask for an engine.

use crate::attached::AttachedBook;
use crate::cloud::Cloud;
use crate::cloud::models::RemoteModel;
use crate::cloud::turn::CloudPin;
use crate::local::{LocalModel, Weights};
use crate::peers::{Caller, Role};
use crate::probed::{ProbedBook, Standing};
use crate::router::{Listed, choose};
use crate::runner::{Pin, Pinned};
use crate::serve::{EngineFailed, EngineHost};
use crate::session::{RouteDecision, Routing, SessionSpec};
use crate::settings::{Live, Settings};
use crate::startup::Cause;
use crate::supervise::{Snapshot, Supervised};
use crate::swap::{Budget, running_of, swap_cost};
use engine_supervisor::{EngineId, EngineState, MonoMs};
use model_catalog::Licence;
use porter_core::capability::SpeechMode;
use porter_core::consent::{Availability, Usage};
use porter_core::{AccountId, AppId, DataClass, Locality, Need, Tier};
use porter_infer::{
    AutoPolicy, InferRefusal, LicenceClass, ModelCard, ModelRef, PickRefusal, Policy, Readiness,
    ServedBy, SpendVerdict, SwapCost, TierMap, Why,
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

/// The account a hosted model is picked under in `ai.model.<kind>.<tier>`: the person picks a
/// model, not an account, and the account that serves it is whichever the app holds a grant on.
/// Routing reads `cloud/<model>` as "this model, through the reach the app is granted".
pub const CLOUD_ACCOUNT: &str = "cloud";

/// The hosted models one session may be routed to: what the app's grants reach, with what the
/// spend caps say about one more request on each.
#[derive(Debug, Clone, Default)]
pub struct Offered {
    models: Vec<(Arc<RemoteModel>, SpendVerdict)>,
    app: Option<AppId>,
}

impl Offered {
    /// The hosted models offered, in catalogue order.
    pub fn models(&self) -> impl Iterator<Item = &Arc<RemoteModel>> {
        self.models.iter().map(|(model, _)| model)
    }

    fn find(&self, model: &ModelRef) -> Option<&Arc<RemoteModel>> {
        self.models().find(|one| one.model_ref() == *model)
    }
}

/// What routing and readiness are computed from.
#[derive(Debug, Default)]
struct Book {
    local: Vec<Arc<LocalModel>>,
    remote: Vec<ModelCard>,
    /// The hosted models of the curated catalogue and how they are reached.
    cloud: Option<Cloud>,
    /// The settings in force; shared by every clone, replaced when the file changes.
    live: Live,
    /// The runtimes the person runs themselves, as the last probe found them; shared by every
    /// clone.
    probed: ProbedBook,
    /// The engines the person already runs; never started, stopped or evicted, looked at when a
    /// session opens.
    attached: AttachedBook,
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
                cloud: None,
                live: Live::new(Settings {
                    policy,
                    tiers,
                    ..Settings::default()
                }),
                probed: ProbedBook::default(),
                attached: AttachedBook::default(),
            }),
            supervised,
        }
    }

    /// The same, with the `ai.auto.*` rows the daemon read (the default rows otherwise).
    pub fn with_auto(self, auto: AutoPolicy) -> Self {
        let settings = Settings {
            auto,
            ..(*self.settings()).clone()
        };
        self.with_settings(settings)
    }

    /// The same, starting from these settings (a new holder: the old one's clones keep theirs).
    pub fn with_settings(self, settings: Settings) -> Self {
        let book = Book {
            local: self.book.local.clone(),
            remote: self.book.remote.clone(),
            cloud: self.book.cloud.clone(),
            live: Live::new(settings),
            probed: self.book.probed.clone(),
            attached: self.book.attached.clone(),
        };
        Self {
            book: Arc::new(book),
            supervised: self.supervised,
        }
    }

    /// The settings in force now.
    pub fn settings(&self) -> Arc<Settings> {
        self.book.live.get()
    }

    /// Puts new settings in force for every clone of these engines: the next session is routed
    /// by them, a session already open keeps the decision it was given.
    pub fn apply(&self, settings: Settings) {
        self.book.live.set(settings);
    }

    /// The same, with models of accounts that are not on this computer. They take part in
    /// routing (so a floor refuses them), but nothing serves a turn from them yet: a route that
    /// lands on one is `Unavailable`.
    pub fn with_remote(self, remote: Vec<ModelCard>) -> Self {
        let book = Book {
            local: self.book.local.clone(),
            remote,
            cloud: self.book.cloud.clone(),
            live: self.book.live.clone(),
            probed: self.book.probed.clone(),
            attached: self.book.attached.clone(),
        };
        Self {
            book: Arc::new(book),
            supervised: self.supervised,
        }
    }

    /// The same, serving the hosted models of the catalogue to the apps that hold a grant on an
    /// account that reaches them.
    pub fn with_cloud(self, cloud: Cloud) -> Self {
        let book = Book {
            local: self.book.local.clone(),
            remote: self.book.remote.clone(),
            cloud: Some(cloud),
            live: self.book.live.clone(),
            probed: self.book.probed.clone(),
            attached: self.book.attached.clone(),
        };
        Self {
            book: Arc::new(book),
            supervised: self.supervised,
        }
    }

    /// The same, using the engines the person attached (`attached::models`). They are in no
    /// supervisor: nothing here starts, stops, evicts or kills them.
    pub fn with_attached(self, attached: AttachedBook) -> Self {
        let book = Book {
            local: self.book.local.clone(),
            remote: self.book.remote.clone(),
            cloud: self.book.cloud.clone(),
            live: self.book.live.clone(),
            probed: self.book.probed.clone(),
            attached,
        };
        Self {
            book: Arc::new(book),
            supervised: self.supervised,
        }
    }

    /// The engines the person attached, and what the last look found of each.
    pub fn attached(&self) -> &AttachedBook {
        &self.book.attached
    }

    /// The runtimes the person runs themselves, as the last probe found them. The probe writes
    /// here (`watch`); routing, the picker and the session machine read.
    pub fn probed(&self) -> &ProbedBook {
        &self.book.probed
    }

    /// The hosted models, when this daemon serves any.
    pub fn cloud(&self) -> Option<&Cloud> {
        self.book.cloud.as_ref()
    }

    /// What `app` has used of hosted models today and this month, by name; empty when this daemon
    /// has none (the `Usage` dictionary of `Inference1`).
    pub fn usage_of(&self, app: &AppId) -> Vec<(String, u64)> {
        match &self.book.cloud {
            Some(cloud) => cloud
                .ledger()
                .usage_of(self.settings().spend, app, cloud.now()),
            None => Vec::new(),
        }
    }

    /// The hosted models `app` is offered for data of `class` now: what its grants reach, and the
    /// caps' verdict on each. None when this daemon has no hosted models or accountd does not
    /// answer.
    pub async fn offer(&self, app: &AppId, class: DataClass, usage: Usage) -> Offered {
        self.look_at_attached().await;
        let Some(cloud) = &self.book.cloud else {
            return Offered::default();
        };
        let line = self.settings().spend;
        let models = cloud
            .models(app, class, usage)
            .await
            .into_iter()
            .map(|model| {
                let spend = cloud.spend(line, app, &model);
                (Arc::new(model), spend)
            })
            .collect();
        Offered {
            models,
            app: Some(app.clone()),
        }
    }

    /// Looks at every engine the person attached, now: what a session is decided by is read from
    /// this look. It is made when a session opens (`offer`, and a router that knows no app) and
    /// nowhere else; nothing polls.
    pub async fn look_at_attached(&self) {
        self.book.attached.reprobe_all().await;
    }

    /// The supervisor handle (the runner marks engines used through it).
    pub fn supervised(&self) -> &Supervised {
        &self.supervised
    }

    fn local(&self, model: &ModelRef) -> Option<Arc<LocalModel>> {
        self.book
            .local
            .iter()
            .find(|one| one.model_ref() == *model)
            .cloned()
            .or_else(|| self.book.probed.find(model))
            .or_else(|| self.book.attached.find(model))
    }

    /// The local model `model` names, when this computer has it (the speech `Hear` stage looks
    /// up the host to run on this way).
    pub fn local_model(&self, model: &ModelRef) -> Option<Arc<LocalModel>> {
        self.local(model)
    }

    /// How ready one local model is now: a supervised one by its engine, a probed one by whether
    /// its runtime answered the last look.
    pub fn readiness(&self, model: &LocalModel) -> Readiness {
        match (&model.attached, model.loopback) {
            (Some(_), _) => self.book.attached.readiness(&model.model_ref()),
            (None, Some(_)) => self.runtime_readiness(model),
            (None, None) => readiness_in(&self.supervised.snapshot(), model),
        }
    }

    fn runtime_readiness(&self, model: &LocalModel) -> Readiness {
        self.book
            .probed
            .standing_of(&model.card.account)
            .map_or(Readiness::Unavailable, Standing::readiness)
    }

    /// Every model a session may be routed to, with its readiness now and what loading it would
    /// take (stoker's `budget`, asked speculatively: nothing is started).
    pub fn listed(&self) -> Vec<Listed> {
        self.listed_with(&Offered::default())
    }

    /// `listed`, and the hosted models `offered` to the app that is asking.
    pub fn listed_with(&self, offered: &Offered) -> Vec<Listed> {
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
        let probed = self.book.probed.models();
        let on_runtime = probed.iter().map(|(model, standing)| {
            Listed::new(
                model.card.clone(),
                standing.readiness(),
                SwapCost::Resident,
                licence_of(&model.entry.licence),
            )
        });
        let attached = self.book.attached.models().iter().map(|model| Listed {
            permission: attached_grant(),
            ..Listed::new(
                model.card.clone(),
                self.book.attached.readiness(&model.model_ref()),
                SwapCost::Resident,
                licence_of(&model.entry.licence),
            )
        });
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
            Listed::new(
                model.card.clone(),
                readiness,
                swap,
                licence_of(&model.entry.licence),
            )
        });
        let remote = self.book.remote.iter().map(|card| {
            Listed::new(
                card.clone(),
                Readiness::Ready,
                SwapCost::Resident,
                LicenceClass::Proprietary,
            )
        });
        let hosted = offered.models.iter().map(|(model, spend)| Listed {
            permission: model.verdict.clone(),
            spend: *spend,
            provider: Some(porter_infer::ProviderId(model.reach.provider.0.clone())),
            ..Listed::new(
                model.card.clone(),
                Readiness::Ready,
                SwapCost::Resident,
                LicenceClass::Proprietary,
            )
        });
        local
            .chain(attached)
            .chain(on_runtime)
            .chain(remote)
            .chain(hosted)
            .collect()
    }

    /// Decides who answers a session, and what the runner is to be pinned to. A need no runner
    /// can serve yet (speaking) and a model that is not on this computer are `Unavailable`.
    pub fn route(
        &self,
        spec: &SessionSpec,
        role: Role,
    ) -> Result<(RouteDecision, Pinned), InferRefusal> {
        self.route_detailed(spec, role)
            .map(|(routing, pinned)| (routing.decision(), pinned))
            .map_err(|r| r.refusal)
    }

    /// `route`, with the reason a named model could not serve when that is why it refused.
    pub fn route_detailed(
        &self,
        spec: &SessionSpec,
        role: Role,
    ) -> Result<(Routing, Pinned), PickRefusal> {
        self.route_with(spec, role, &Offered::default())
    }

    /// `route_detailed` for an app that is offered the hosted models in `offered`.
    pub fn route_with(
        &self,
        spec: &SessionSpec,
        role: Role,
        offered: &Offered,
    ) -> Result<(Routing, Pinned), PickRefusal> {
        if matches!(spec.need, Need::ComputerUse(_)) && role != Role::Cua {
            return Err(InferRefusal::Denied.into());
        }
        let settings = self.settings();
        let tiers = through_grants(&settings.tiers, offered);
        let decided = choose(
            &spec.need,
            spec.class,
            spec.tier,
            &self.listed_for(&spec.need, offered),
            &settings.routing_policy(),
            &tiers,
            settings.auto,
        )?;
        let (chosen, readiness) = (decided.chosen, decided.readiness);
        let chosen_ref = ModelRef {
            account: chosen.account.clone(),
            model: chosen.model.clone(),
        };
        let (locality, model, cloud) = self
            .backing(&chosen_ref, offered, &settings)
            .ok_or_else(|| PickRefusal::from(InferRefusal::Unavailable))?;
        let name = self.label_of(&chosen_ref, offered);
        let served = ServedBy {
            account: chosen.account,
            model: chosen.model,
            locality,
        };
        Ok((
            Routing {
                served: served.clone(),
                readiness,
                why: decided.why,
                reached: cloud.as_ref().map(|pin| reached_of(&pin.model.reach)),
                show: settings.auto.show_reason,
                name,
            },
            Pinned {
                served,
                model,
                cloud,
            },
        ))
    }

    /// What runs `model` for a session: where it is (`Locality`), the local model behind it, or the
    /// hosted one (for the app `offered` is for). None for a model nothing here can run.
    fn backing(
        &self,
        model: &ModelRef,
        offered: &Offered,
        settings: &Settings,
    ) -> Option<(Locality, Option<Arc<LocalModel>>, Option<CloudPin>)> {
        match (self.local(model), offered.find(model)) {
            (Some(local), _) => Some((local.card.locality.clone(), Some(local), None)),
            (None, Some(hosted)) => Some((
                hosted.card.locality.clone(),
                None,
                offered.app.clone().map(|app| CloudPin {
                    model: Arc::clone(hosted),
                    app,
                    line: settings.spend,
                }),
            )),
            (None, None) => None,
        }
    }

    /// The catalogue label of `model`, local or hosted: what a person reads as its name.
    pub fn label_of(
        &self,
        model: &ModelRef,
        offered: &Offered,
    ) -> Option<porter_infer::ModelLabel> {
        self.local(model)
            .map(|local| local.entry.label.clone())
            .or_else(|| offered.find(model).map(|hosted| hosted.entry.label.clone()))
            .map(porter_infer::ModelLabel)
    }

    /// What a turn on `model` is pinned to, for a model that is not the one a session's route
    /// chose (the answering stage of a pipeline). None for a model nothing here can run.
    pub fn pinned_for(&self, model: &ModelRef, offered: &Offered) -> Option<Pinned> {
        let (locality, local, cloud) = self.backing(model, offered, &self.settings())?;
        Some(Pinned {
            served: ServedBy {
                account: model.account.clone(),
                model: model.model.clone(),
                locality,
            },
            model: local,
            cloud,
        })
    }

    /// Models for a need; none when no runner serves that kind of need yet: speech to text runs
    /// on the speech host, speaking has no runner.
    fn listed_for(&self, need: &Need, offered: &Offered) -> Vec<Listed> {
        match need {
            Need::Speech(speech) if !speech.modes.contains(&SpeechMode::Stt) => Vec::new(),
            _ => self.listed_with(offered),
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
        self.prepare_with(
            need,
            class,
            tier,
            Usage::Interactive,
            role,
            &Offered::default(),
        )
        .await
    }

    /// `prepare` for this caller: the hosted models its grants reach are among the candidates (a
    /// hosted model is always ready, so there is nothing to warm for one).
    pub async fn prepare_for(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        usage: Usage,
        caller: &Caller,
    ) -> Result<Readiness, InferRefusal> {
        let offered = self.offer(&caller.app, class, usage).await;
        self.prepare_with(need, class, tier, usage, caller.role, &offered)
            .await
    }

    async fn prepare_with(
        &self,
        need: &Need,
        class: DataClass,
        tier: Tier,
        usage: Usage,
        role: Role,
        offered: &Offered,
    ) -> Result<Readiness, InferRefusal> {
        let spec = SessionSpec {
            need: need.clone(),
            class,
            tier,
            usage,
        };
        let (decision, pinned) = self
            .route_with(&spec, role, offered)
            .map(|(routing, pinned)| (routing.decision(), pinned))
            .map_err(|r| r.refusal)?;
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
        self.availability_with(need, class, Usage::Interactive, role, &Offered::default())
    }

    /// `availability` for this caller: the hosted models its grants reach are among the candidates.
    pub async fn availability_for(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
        caller: &Caller,
    ) -> Availability {
        let offered = self.offer(&caller.app, class, usage).await;
        self.availability_with(need, class, usage, caller.role, &offered)
    }

    fn availability_with(
        &self,
        need: &Need,
        class: DataClass,
        usage: Usage,
        role: Role,
        offered: &Offered,
    ) -> Availability {
        let spec = SessionSpec {
            need: need.clone(),
            class,
            tier: Tier::Balanced,
            usage,
        };
        match self.route_with(&spec, role, offered).map_err(|r| r.refusal) {
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
            app: None,
        }
    }

    /// The router for one session of `caller`: as `router`, and the hosted models its grants reach
    /// are among the candidates. (A router made by `router` knows no app and serves none.)
    pub fn router_for(&self, pin: Pin, caller: &Caller) -> SessionRouter {
        SessionRouter {
            app: Some(caller.app.clone()),
            ..self.router(pin, caller.role)
        }
    }
}

/// How a hosted model is reached, as the footer says it ("via OpenRouter").
fn reached_of(reach: &model_catalog::Reach) -> Why {
    Why::Reached {
        provider: porter_infer::ProviderId(reach.provider.0.clone()),
        door: if reach.is_via_gateway() {
            porter_infer::Door::Gateway
        } else {
            porter_infer::Door::Direct
        },
    }
}

/// The verdict an attached engine carries: the person named it, so no grant is asked of accountd
/// (where its data goes is `where`, which the class floor reads).
fn attached_grant() -> porter_core::consent::Verdict {
    match porter_core::GrantId::parse("attached-engine") {
        Ok(grant) => porter_core::consent::Verdict::Granted {
            grant,
            scope: porter_core::consent::GrantScope::Always,
        },
        Err(_) => porter_core::consent::Verdict::Ask,
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
        // An engine that failed to start is unavailable for a pause (its failure is the answer), and
        // loadable again when the pause has run out: a request then tries it once more.
        Some(EngineState::Failed(_)) => match snapshot.failures.get(&model.spec.id) {
            Some(failure) if failure.cause.pauses() && snapshot.now >= Some(failure.retry_at) => {
                Readiness::Loadable
            }
            _ => Readiness::Unavailable,
        },
        None => Readiness::Unavailable,
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
        let Some(local) = self.local(&model) else {
            // A hosted model has no engine to start: the route said it is ready.
            let hosted = self.book.cloud.as_ref().is_some_and(|cloud| {
                cloud
                    .entries()
                    .iter()
                    .any(|entry| entry.id.0 == model.model.as_str())
            });
            return if hosted {
                Ok(())
            } else {
                Err(EngineFailed::unknown())
            };
        };
        // An attached engine is looked at, never started: it answers or it says why not.
        if local.attached.is_some() {
            return self
                .book
                .attached
                .reprobe(&model)
                .await
                .map_err(|why| EngineFailed {
                    cause: Cause::Attached(why),
                });
        }
        // A runtime the person runs is not started or stopped here: it is there or it is not.
        if local.loopback.is_some() {
            return match self.runtime_readiness(&local) {
                Readiness::Ready => Ok(()),
                _ => Err(EngineFailed::unknown()),
            };
        }
        self.supervised
            .want(&local.spec.id)
            .await
            .map_err(|failed| EngineFailed {
                cause: failed.cause,
            })
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
    app: Option<AppId>,
}

impl SessionRouter {
    /// What the session's app is offered: the hosted models its grants reach, when it is known.
    async fn offered(&self, spec: &SessionSpec) -> Offered {
        match &self.app {
            Some(app) => self.engines.offer(app, spec.class, spec.usage).await,
            None => {
                self.engines.look_at_attached().await;
                Offered::default()
            }
        }
    }
}

impl crate::serve::Router for SessionRouter {
    async fn route(&self, spec: &SessionSpec) -> Result<RouteDecision, InferRefusal> {
        self.route_why(spec)
            .await
            .map(|routing| routing.decision())
            .map_err(|r| r.refusal)
    }

    async fn route_why(&self, spec: &SessionSpec) -> Result<Routing, PickRefusal> {
        let offered = self.offered(spec).await;
        let (routing, pinned) = self.engines.route_with(spec, self.role, &offered)?;
        self.pin.set(pinned);
        Ok(routing)
    }
}

/// The tier map with every `cloud/<model>` row (a hosted model picked without an account) naming
/// the account that reaches it for this app; a row nothing reaches stays as it is, and so names a
/// model no candidate has.
pub(crate) fn through_grants(tiers: &TierMap, offered: &Offered) -> TierMap {
    let virtual_account = AccountId::parse(CLOUD_ACCOUNT).ok();
    let resolve = |model: &ModelRef| match offered.models().find(|one| {
        Some(&model.account) == virtual_account.as_ref() && one.card.model == model.model
    }) {
        Some(hosted) => hosted.model_ref(),
        None => model.clone(),
    };
    TierMap {
        rows: tiers
            .rows
            .iter()
            .map(|row| porter_infer::TierRow {
                model: resolve(&row.model),
                ..row.clone()
            })
            .collect(),
        autos: tiers.autos.clone(),
    }
}

#[cfg(test)]
mod tests;
