//! Who answers a session: the models that meet the need, minus what the policy forbids, then
//! `porter_infer::route` (this computer first, the user's tier choice, cost). Pure over the list
//! of models and their readiness; the engines and the clock are somebody else's.
//!
//! Consent: a model on this computer needs no grant (the data does not leave the machine, and
//! the class's floor already decides which classes may be sent where). A hosted model carries the
//! verdict accountd gave the app for its account (`Listed::permission`, from `cloud::models`); a
//! model of an account inferd knows nothing of is `Ask` (`NeedsGrant`).

use crate::cua_run::check_class;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{DataClass, GrantId, Locality, Need, Tier};
use porter_core::{Match, matches};
use porter_core::{Offer as CoreOffer, capability::SpeechMode};
use porter_infer::{
    AiKind, AutoPolicy, Chosen, InferRefusal, LicenceClass, ModelCard, ModelRef, PickCandidate,
    PickPolicy, PickRefusal, Policy, Readiness, RouteAsk, RouteCandidate, SpendVerdict, SwapCost,
    TierMap, Why, pick, route, tier_choice,
};

/// One model the router may pick, and how soon it can answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// What routing knows about it.
    pub card: ModelCard,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// What loading it now would take (`swap::swap_cost`); `Resident` for a model that is not
    /// ours to load.
    pub swap: SwapCost,
    /// How it may be used.
    pub licence: LicenceClass,
    /// What the consent store says for the app on this model's account.
    pub permission: Verdict,
    /// What the spend caps say about one more request on it.
    pub spend: SpendVerdict,
}

impl Listed {
    /// A model with the consent its locality carries (`consent_of`) and no spend against it.
    pub fn new(
        card: ModelCard,
        readiness: Readiness,
        swap: SwapCost,
        licence: LicenceClass,
    ) -> Self {
        let permission = consent_of(&card);
        Self {
            card,
            readiness,
            swap,
            licence,
            permission,
            spend: SpendVerdict::Within,
        }
    }
}

/// What the router decided: the model, how ready it is, and why it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decided {
    /// The model and the spend verdict.
    pub chosen: Chosen,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// Why this model.
    pub why: Why,
}

/// The kind of AI work a need is, for the user's tier map.
pub fn ai_kind(need: &Need) -> Option<AiKind> {
    match need {
        Need::Llm(_) => Some(AiKind::Llm),
        Need::ComputerUse(_) => Some(AiKind::ComputerUse),
        Need::Embeddings(_) => Some(AiKind::Embeddings),
        Need::Speech(speech) if speech.modes.contains(&SpeechMode::Tts) => Some(AiKind::SpeechOut),
        Need::Speech(_) => Some(AiKind::SpeechIn),
        Need::ImageGen(_) => Some(AiKind::ImageGen),
        Need::Rerank(_) => Some(AiKind::Rerank),
        _ => None,
    }
}

/// Whether a model's readiness lets a session wait for it: one that is not installed or not
/// available cannot answer, and there is no downloader to make it.
fn can_serve(readiness: Readiness) -> bool {
    !matches!(readiness, Readiness::Downloadable | Readiness::Unavailable)
}

/// The grant the on-device models carry: one name for all of them.
fn local_grant() -> Option<GrantId> {
    GrantId::parse("on-this-computer").ok()
}

/// What the consent store says for `card` when nobody has said: a model on this computer is
/// granted, any other asks.
pub(crate) fn consent_of(card: &ModelCard) -> Verdict {
    match (&card.locality, local_grant()) {
        (Locality::OnDevice, Some(grant)) => Verdict::Granted {
            grant,
            scope: GrantScope::Always,
        },
        _ => Verdict::Ask,
    }
}

fn fits(need: &Need, card: &ModelCard) -> bool {
    card.capabilities
        .iter()
        .any(|capability| matches(need, &CoreOffer::Present(capability.clone())) == Match::Fits)
}

/// The model to run for a session of this need, class and tier, and its readiness; or why none
/// may. A computer-use need with any class but `Screen` is `Unsupported`, as the session
/// machine would answer it later.
///
/// The person's pick for the kind and tier decides how: a named model is served or refused
/// (never replaced), "auto" is `porter_infer::pick`, and an empty row is the catalogue's own
/// choice, as it always was (`route`).
pub fn choose(
    need: &Need,
    class: DataClass,
    tier: Tier,
    listed: &[Listed],
    policy: &Policy,
    tiers: &TierMap,
    auto: AutoPolicy,
) -> Result<Decided, PickRefusal> {
    if matches!(need, Need::ComputerUse(_)) {
        check_class(class).map_err(plain)?;
    }
    let kind = ai_kind(need).ok_or(plain(InferRefusal::Unsupported))?;
    let fitting: Vec<&Listed> = listed.iter().filter(|one| fits(need, &one.card)).collect();
    let candidates: Vec<PickCandidate> = fitting
        .iter()
        .enumerate()
        .map(|(index, one)| PickCandidate {
            route: RouteCandidate {
                account: one.card.account.clone(),
                model: one.card.model.clone(),
                locality: one.card.locality.clone(),
                billing: one.card.billing.clone(),
                tier: tier_choice(
                    tiers,
                    kind,
                    tier,
                    &ModelRef {
                        account: one.card.account.clone(),
                        model: one.card.model.clone(),
                    },
                ),
                permission: one.permission.clone(),
                spend: one.spend,
            },
            readiness: one.readiness,
            swap: one.swap.clone(),
            licence: one.licence,
            catalogue_index: u32::try_from(index).unwrap_or(u32::MAX),
        })
        .collect();
    let ask = RouteAsk { class };
    match tiers.pick(kind, tier) {
        Some(chosen_pick) => {
            let rules = PickPolicy { policy, auto };
            pick(ask, &chosen_pick, &candidates, rules).map(|picked| Decided {
                chosen: picked.chosen,
                readiness: picked.readiness,
                why: picked.why,
            })
        }
        None => catalogue_choice(ask, &candidates, policy),
    }
}

fn plain(refusal: InferRefusal) -> PickRefusal {
    PickRefusal {
        refusal,
        declined: None,
    }
}

/// An empty row: what `route` has always chosen among the models that can answer.
fn catalogue_choice(
    ask: RouteAsk,
    candidates: &[PickCandidate],
    policy: &Policy,
) -> Result<Decided, PickRefusal> {
    let usable: Vec<&PickCandidate> = candidates
        .iter()
        .filter(|one| can_serve(one.readiness))
        .collect();
    let routes: Vec<RouteCandidate> = usable.iter().map(|one| one.route.clone()).collect();
    let chosen = route(ask, &routes, policy).map_err(plain)?;
    let readiness = usable
        .iter()
        .find(|one| one.route.account == chosen.account && one.route.model == chosen.model)
        .map_or(Readiness::Unavailable, |one| one.readiness);
    let why = if usable.len() == 1 {
        Why::OnlyOne
    } else {
        Why::CatalogueOrder
    };
    Ok(Decided {
        chosen,
        readiness,
        why,
    })
}

#[cfg(test)]
mod tests;
