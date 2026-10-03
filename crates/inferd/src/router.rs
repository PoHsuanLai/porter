//! Who answers a session: the models that meet the need, minus what the policy forbids, then
//! `porter_infer::route` (this computer first, the user's tier choice, cost). Pure over the list
//! of models and their readiness; the engines and the clock are somebody else's.
//!
//! Consent: a model on this computer needs no grant (the data does not leave the machine, and
//! the class's floor already decides which classes may be sent where); any other model needs a
//! grant that only accountd can give, which it cannot yet, so it answers `Ask` (`NeedsGrant`).

use crate::cua_run::check_class;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{DataClass, GrantId, Locality, Need, Tier};
use porter_core::{Match, matches};
use porter_core::{Offer as CoreOffer, capability::SpeechMode};
use porter_infer::{
    AiKind, Chosen, InferRefusal, ModelCard, ModelRef, Policy, Readiness, RouteAsk, RouteCandidate,
    SpendVerdict, TierMap, route, tier_choice,
};

/// One model the router may pick, and how soon it can answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// What routing knows about it.
    pub card: ModelCard,
    /// Whether it can answer now.
    pub readiness: Readiness,
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

/// What the consent store says for `card`, today.
fn consent_of(card: &ModelCard) -> Verdict {
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
pub fn choose(
    need: &Need,
    class: DataClass,
    tier: Tier,
    listed: &[Listed],
    policy: &Policy,
    tiers: &TierMap,
) -> Result<(Chosen, Readiness), InferRefusal> {
    if matches!(need, Need::ComputerUse(_)) {
        check_class(class)?;
    }
    let kind = ai_kind(need).ok_or(InferRefusal::Unsupported)?;
    let usable: Vec<&Listed> = listed
        .iter()
        .filter(|one| fits(need, &one.card) && can_serve(one.readiness))
        .collect();
    let candidates: Vec<RouteCandidate> = usable
        .iter()
        .map(|one| RouteCandidate {
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
            permission: consent_of(&one.card),
            spend: SpendVerdict::Within,
        })
        .collect();
    let chosen = route(RouteAsk { class }, &candidates, policy)?;
    let readiness = usable
        .iter()
        .find(|one| one.card.account == chosen.account && one.card.model == chosen.model)
        .map_or(Readiness::Unavailable, |one| one.readiness);
    Ok((chosen, readiness))
}

#[cfg(test)]
mod tests;
