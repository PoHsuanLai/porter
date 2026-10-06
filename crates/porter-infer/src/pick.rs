//! Picking a model for a session (research-routing-fit P0): the person's pick for the kind and
//! tier, a named model or Automatic, resolved over candidates that carry the engines' state.
//!
//! `pick` is pure: no IO, no clock, no randomness. What it knows about the engines (warm, idle or
//! mid-turn, what a swap would cost) arrives as plain data in [`PickCandidate`]; the swap cost is
//! worked out by the caller from stoker's pure `budget`. The hard rules (`ai.local_only`, the
//! class floor, consent, spend) are [`admit`]'s and only its: every answer here comes out of
//! `admit` first, so no pick can serve what `route` would refuse.
//!
//! * A named model is never overridden. If it cannot serve, the answer is a refusal that names it
//!   and says why ([`Declined`]); there is no fallback.
//! * Automatic orders what is left by: nearer first (this computer, the network, the cloud, so it
//!   chooses the cloud only when `admit` allows it and nothing nearer can serve), then models
//!   already loaded, then the cheaper swap, then the lower price, then the catalogue's order. It
//!   may unload one idle engine to load another, only when `ai.auto.allow_evict` says
//!   [`AutoEvict::IdleOnly`]; an engine in a turn is never a victim.
//! * Every answer carries a [`Why`], a closed enum of facts, never a judgement.

use crate::choice::{LicenceClass, ModelRef};
use crate::error::InferRefusal;
use crate::policy::Policy;
use crate::readiness::Readiness;
use crate::route::{Chosen, RouteAsk, RouteCandidate, admit, chosen_of, closeness, price_rank};
use serde::{Deserialize, Serialize};

/// What Automatic does (`ai.auto.mode`). One value in P0; the enum leaves room for more.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoMode {
    /// Among the models allowed, prefer one that is already loaded.
    #[default]
    WarmFirst,
}

impl AutoMode {
    /// The settings value.
    pub fn slug(self) -> &'static str {
        match self {
            AutoMode::WarmFirst => "warm_first",
        }
    }

    /// The mode a settings value names.
    pub fn from_slug(slug: &str) -> Option<Self> {
        [AutoMode::WarmFirst]
            .into_iter()
            .find(|mode| mode.slug() == slug)
    }
}

/// Whether Automatic may unload a model to load another (`ai.auto.allow_evict`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoEvict {
    /// Stay with what is loaded or what fits beside it.
    Never,
    /// May unload an idle engine; never one in a turn.
    #[default]
    IdleOnly,
}

impl AutoEvict {
    /// The settings value.
    pub fn slug(self) -> &'static str {
        match self {
            AutoEvict::Never => "never",
            AutoEvict::IdleOnly => "idle_only",
        }
    }

    /// The value a settings string names.
    pub fn from_slug(slug: &str) -> Option<Self> {
        [AutoEvict::Never, AutoEvict::IdleOnly]
            .into_iter()
            .find(|one| one.slug() == slug)
    }
}

/// Whether the reason is announced to the client (`ai.auto.show_reason`). It is always audited;
/// an eviction is announced whatever this says (a swap is never hidden).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowReason {
    /// Only an eviction is announced.
    Off,
    /// Every answer says why.
    #[default]
    On,
}

impl ShowReason {
    /// The settings value.
    pub fn slug(self) -> &'static str {
        match self {
            ShowReason::Off => "off",
            ShowReason::On => "on",
        }
    }

    /// The value a settings string names.
    pub fn from_slug(slug: &str) -> Option<Self> {
        [ShowReason::Off, ShowReason::On]
            .into_iter()
            .find(|one| one.slug() == slug)
    }
}

/// The `ai.auto.*` rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AutoPolicy {
    /// `ai.auto.mode`.
    pub mode: AutoMode,
    /// `ai.auto.allow_evict`.
    pub allow_evict: AutoEvict,
    /// `ai.auto.show_reason`.
    pub show_reason: ShowReason,
}

/// The person's choice for a kind and tier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Pick {
    /// This model, exactly.
    Named(ModelRef),
    /// Choose for me.
    Auto(AutoMode),
}

/// What an engine is doing now, for the one that would be unloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineLoad {
    /// Not answering anything now: may be unloaded.
    Idle,
    /// In a turn: never unloaded.
    MidTurn,
}

/// What loading a candidate now would take, from the engines' state and the memory budget.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SwapCost {
    /// It is loaded or loading: nothing to do.
    Resident,
    /// It fits beside what is loaded. The figure is the catalogue's estimate, not a measurement.
    Fits {
        /// Estimated seconds from cold to ready.
        cold_start_estimate_s: u16,
    },
    /// One engine must be unloaded first.
    Evicts {
        /// The engine that would be unloaded.
        victim: ModelRef,
        /// What that engine is doing now.
        load: EngineLoad,
        /// Estimated seconds from cold to ready.
        cold_start_estimate_s: u16,
    },
    /// More than one engine would have to go: Automatic never does that.
    Purge {
        /// How many engines.
        engines: u8,
    },
    /// Not even with every idle engine unloaded.
    NoRoom,
}

/// One model the pick may land on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickCandidate {
    /// What the hard rules read.
    pub route: RouteCandidate,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// What loading it would take.
    pub swap: SwapCost,
    /// How it may be used; a non-commercial model is never chosen by Automatic.
    pub licence: LicenceClass,
    /// Its place in the catalogue's directory order: the last tie-break, so the answer does not
    /// depend on the order of the slice.
    pub catalogue_index: u32,
}

impl PickCandidate {
    /// The model it is.
    pub fn model_ref(&self) -> ModelRef {
        ModelRef {
            account: self.route.account.clone(),
            model: self.route.model.clone(),
        }
    }
}

/// Why this model, as a fact about the request and the machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Why {
    /// The person named it.
    Named,
    /// It is the only one that can do the request.
    OnlyOne,
    /// It runs on this computer or the network, and the others do not.
    Nearest,
    /// It is already loaded.
    Warm,
    /// Loading it costs less than loading the others.
    Smallest,
    /// The others cost more to use.
    LeastCost,
    /// Nothing told them apart; the catalogue lists it first.
    CatalogueOrder,
    /// The pick named another model that could not serve and the person allowed a fallback.
    FallbackFrom {
        /// The model that could not serve.
        model: ModelRef,
    },
    /// It is loading, and `model` is being unloaded for it.
    Evicted {
        /// The idle model that is unloaded.
        model: ModelRef,
    },
    /// A hosted model is reached through this provider, by this door ("via OpenRouter"). Appended
    /// after the others: a reader that does not know it must skip it.
    Reached {
        /// The provider's id (`openrouter`, `anthropic`, ...).
        provider: crate::pipeline::ProviderId,
        /// Whether the provider is the model's own company or a gateway.
        door: Door,
    },
}

/// How a hosted model is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Door {
    /// The model's own company's account.
    Direct,
    /// A gateway that reaches many companies (OpenRouter).
    Gateway,
}

/// What a pick resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// The model, and the spend verdict to carry.
    pub chosen: Chosen,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// Why this one.
    pub why: Why,
}

/// Why a named model could not serve.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeclinedBecause {
    /// No account offers it for this request.
    NotListed,
    /// The hard rules refuse it (`ai.local_only`, the floor, consent, spend).
    Blocked {
        /// The refusal `admit` gave.
        refusal: InferRefusal,
    },
    /// Its weights are not on this computer.
    NotInstalled,
    /// It cannot run at all now.
    Unavailable,
    /// It does not fit in memory, even with every idle engine unloaded.
    NoRoom,
}

/// A named model that cannot serve, and why. Shown to the person; never answered by another model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Declined {
    /// The model the person named.
    pub model: ModelRef,
    /// Why it cannot serve.
    pub because: DeclinedBecause,
}

/// A pick that resolved to nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRefusal {
    /// The typed refusal the session ends with.
    pub refusal: InferRefusal,
    /// Set when the pick named a model: which, and why it could not serve.
    pub declined: Option<Declined>,
}

impl From<InferRefusal> for PickRefusal {
    fn from(refusal: InferRefusal) -> Self {
        Self {
            refusal,
            declined: None,
        }
    }
}

/// What a pick needs beyond the candidates.
#[derive(Debug, Clone, Copy)]
pub struct PickPolicy<'a> {
    /// `ai.local_only` and the floors.
    pub policy: &'a Policy,
    /// The `ai.auto.*` rows.
    pub auto: AutoPolicy,
}

/// The model to run for `ask` under the person's `pick`, or why none may.
pub fn pick(
    ask: RouteAsk,
    pick: &Pick,
    candidates: &[PickCandidate],
    rules: PickPolicy<'_>,
) -> Result<Picked, PickRefusal> {
    match pick {
        Pick::Named(model) => named(ask, model, candidates, rules.policy),
        Pick::Auto(AutoMode::WarmFirst) => auto(ask, candidates, rules),
    }
}

fn picked(candidate: &PickCandidate, why: Why) -> Picked {
    Picked {
        chosen: chosen_of(&candidate.route),
        readiness: candidate.readiness,
        why,
    }
}

fn declined(model: &ModelRef, because: DeclinedBecause, refusal: InferRefusal) -> PickRefusal {
    PickRefusal {
        refusal,
        declined: Some(Declined {
            model: model.clone(),
            because,
        }),
    }
}

fn named(
    ask: RouteAsk,
    model: &ModelRef,
    candidates: &[PickCandidate],
    policy: &Policy,
) -> Result<Picked, PickRefusal> {
    let Some(found) = candidates.iter().find(|c| c.model_ref() == *model) else {
        return Err(declined(
            model,
            DeclinedBecause::NotListed,
            InferRefusal::Unavailable,
        ));
    };
    if let Err(refusal) = admit(ask, [&found.route], policy) {
        return Err(declined(
            model,
            DeclinedBecause::Blocked { refusal },
            refusal,
        ));
    }
    let blocked = match (&found.readiness, &found.swap) {
        (Readiness::Downloadable | Readiness::Downloading(_), _) => {
            Some(DeclinedBecause::NotInstalled)
        }
        (Readiness::Unavailable, _) => Some(DeclinedBecause::Unavailable),
        (_, SwapCost::NoRoom) => Some(DeclinedBecause::NoRoom),
        _ => None,
    };
    match blocked {
        Some(because) => Err(declined(model, because, InferRefusal::Unavailable)),
        None => Ok(picked(found, Why::Named)),
    }
}

/// Whether Automatic may load this candidate at all.
fn eligible(candidate: &PickCandidate, rules: PickPolicy<'_>) -> bool {
    let can_answer = matches!(
        candidate.readiness,
        Readiness::Ready | Readiness::Loading | Readiness::Loadable
    );
    let open_licence = candidate.licence != LicenceClass::NonCommercial;
    let swap_allowed = match (&candidate.swap, rules.auto.allow_evict) {
        (SwapCost::Resident | SwapCost::Fits { .. }, _) => true,
        (
            SwapCost::Evicts {
                load: EngineLoad::Idle,
                ..
            },
            AutoEvict::IdleOnly,
        ) => true,
        (SwapCost::Evicts { .. }, _) | (SwapCost::Purge { .. } | SwapCost::NoRoom, _) => false,
    };
    can_answer && open_licence && swap_allowed
}

/// The ordering key of Automatic: nearer, then loaded, then the cheaper swap, then the lower
/// price, then the catalogue's order. Smaller is first.
type Key = (u8, u8, (u8, u16), (u8, u64), u32);

fn key(candidate: &PickCandidate) -> Key {
    let warmth = match candidate.readiness {
        Readiness::Ready => 0,
        Readiness::Loading => 1,
        _ => 2,
    };
    let swap = match &candidate.swap {
        SwapCost::Resident => (0, 0),
        SwapCost::Fits {
            cold_start_estimate_s,
        } => (1, *cold_start_estimate_s),
        SwapCost::Evicts {
            cold_start_estimate_s,
            ..
        } => (2, *cold_start_estimate_s),
        SwapCost::Purge { .. } | SwapCost::NoRoom => (3, u16::MAX),
    };
    (
        closeness(&candidate.route.locality),
        warmth,
        swap,
        price_rank(&candidate.route.billing),
        candidate.catalogue_index,
    )
}

fn auto(
    ask: RouteAsk,
    candidates: &[PickCandidate],
    rules: PickPolicy<'_>,
) -> Result<Picked, PickRefusal> {
    let usable: Vec<&PickCandidate> = candidates
        .iter()
        .filter(|candidate| eligible(candidate, rules))
        .collect();
    let admitted =
        admit(ask, usable.iter().map(|c| &c.route), rules.policy).map_err(PickRefusal::from)?;
    let mut ranked: Vec<&PickCandidate> = usable
        .into_iter()
        .filter(|c| admitted.iter().any(|a| std::ptr::eq(*a, &c.route)))
        .collect();
    ranked.sort_by_key(|c| key(c));
    let (first, rest) = ranked
        .split_first()
        .ok_or(PickRefusal::from(InferRefusal::Unavailable))?;
    Ok(picked(first, why_auto(first, rest.first().copied())))
}

/// The first part of the key in which the answer differs from the runner-up, as a fact.
fn why_auto(first: &PickCandidate, runner_up: Option<&PickCandidate>) -> Why {
    if let SwapCost::Evicts { victim, .. } = &first.swap {
        return Why::Evicted {
            model: victim.clone(),
        };
    }
    let Some(second) = runner_up else {
        return Why::OnlyOne;
    };
    let (a, b) = (key(first), key(second));
    if a.0 != b.0 {
        Why::Nearest
    } else if a.1 != b.1 {
        Why::Warm
    } else if a.2 != b.2 {
        Why::Smallest
    } else if a.3 != b.3 {
        Why::LeastCost
    } else {
        Why::CatalogueOrder
    }
}

#[cfg(test)]
mod props;
#[cfg(test)]
mod tests;
