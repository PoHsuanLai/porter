//! Who answers a session: the models that meet the need, minus what the policy forbids, then
//! `porter_infer::route` (this computer first, the user's tier choice, cost). Pure over the list
//! of models and their readiness; the engines and the clock are somebody else's.
//!
//! Consent: a model on this computer needs no grant (the data does not leave the machine, and
//! the class's floor already decides which classes may be sent where). A hosted model carries the
//! verdict accountd gave the app for its account (`Listed::permission`, from `cloud::models`); a
//! model of an account inferd knows nothing of is `Ask` (`NeedsGrant`).
//!
//! [`placed`] routes inside a set of places a caller named.

pub mod placed;

use porter_core::capability::LlmFeature;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{Capability, DataClass, GrantId, Locality, Need, Tier};
use porter_core::{Match, matches};
use porter_core::{Offer as CoreOffer, capability::SpeechMode};
use porter_infer::{
    AutoPolicy, Chosen, InferRefusal, LicenceClass, ModelCard, ModelRef, PickCandidate, PickPolicy,
    PickRefusal, Policy, ProviderId, Readiness, RouteAsk, RouteCandidate, Slot, SpendVerdict,
    SwapCost, TierMap, Why, default_choice, pick, tier_choice,
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
    /// The provider that reaches it, for a hosted model; none for one on this computer.
    pub provider: Option<ProviderId>,
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
            provider: None,
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

/// The slot a need is served from, for the user's picks.
pub fn slot_of_need(need: &Need) -> Option<Slot> {
    match need {
        Need::Llm(_) => Some(Slot::Text),
        Need::ComputerUse(_) => Some(Slot::ComputerUse),
        Need::Embeddings(_) => Some(Slot::Embeddings),
        Need::Speech(speech) if speech.modes.contains(&SpeechMode::Tts) => Some(Slot::VoiceOut),
        Need::Speech(_) => Some(Slot::VoiceIn),
        Need::ImageGen(_) => Some(Slot::ImageGen),
        Need::Rerank(_) => Some(Slot::Rerank),
        _ => None,
    }
}

/// The slots a model serves, from its capabilities: a language model is in `text`, and in
/// `image_in` when it takes images and `voice_in` when it takes audio; a computer-use model reads
/// images too.
pub fn slots_of(card: &ModelCard) -> Vec<Slot> {
    let mut slots: Vec<Slot> = card
        .capabilities
        .iter()
        .flat_map(|capability| match capability {
            Capability::Llm(llm) => {
                let extra = [
                    (LlmFeature::Vision, Slot::ImageIn),
                    (LlmFeature::AudioIn, Slot::VoiceIn),
                ]
                .into_iter()
                .filter(|(feature, _)| llm.features.contains(feature))
                .map(|(_, slot)| slot);
                std::iter::once(Slot::Text).chain(extra).collect()
            }
            Capability::ComputerUse(_) => vec![Slot::ComputerUse, Slot::ImageIn],
            Capability::Embeddings(_) => vec![Slot::Embeddings],
            Capability::ImageGen(_) => vec![Slot::ImageGen],
            Capability::Rerank(_) => vec![Slot::Rerank],
            Capability::Speech(speech) => [
                (SpeechMode::Stt, Slot::VoiceIn),
                (SpeechMode::Tts, Slot::VoiceOut),
            ]
            .into_iter()
            .filter(|(mode, _)| speech.modes.contains(mode))
            .map(|(_, slot)| slot)
            .collect(),
            _ => Vec::new(),
        })
        .collect();
    slots.sort();
    slots.dedup();
    slots
}

/// Only `Screen` data may enter a computer-use session: the frames are the screen.
pub fn check_class(class: DataClass) -> Result<(), InferRefusal> {
    match class {
        DataClass::Screen => Ok(()),
        _ => Err(InferRefusal::Unsupported),
    }
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

/// Whether any capability of `card` meets `need`.
pub fn fits(need: &Need, card: &ModelCard) -> bool {
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
    let kind = slot_of_need(need).ok_or(plain(InferRefusal::Unsupported))?;
    if let Need::Llm(_) = need {
        // The language slot is planned as a pipeline: today a text-only request, so one stage.
        return crate::pipeline::decide_text(need, class, tier, listed, policy, tiers, auto);
    }
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
    let ask = RouteAsk::new(class);
    let picked = match tiers.pick(kind, tier) {
        Some(chosen_pick) => {
            let rules = PickPolicy { policy, auto };
            pick(ask, &chosen_pick, &candidates, rules)?
        }
        None => default_choice(ask, &candidates, policy)?,
    };
    Ok(Decided {
        chosen: picked.chosen,
        readiness: picked.readiness,
        why: picked.why,
    })
}

pub(crate) fn plain(refusal: InferRefusal) -> PickRefusal {
    PickRefusal {
        refusal,
        declined: None,
    }
}

#[cfg(test)]
mod tests;
