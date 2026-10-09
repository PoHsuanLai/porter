//! The planning half of the pipelines (capabilities.md sections 3 and 4): the catalogue the
//! planner reads, the plan for a request, and the route of a language session.
//!
//! `porter_infer::plan_pipeline` is pure; this is its edge. [`catalogue_of`] turns what routing
//! lists into [`CatalogueModel`]s (what each takes and gives, the slots it is in, the provider that
//! reaches it), [`granted_of`] is the set of providers the person's accounts reach through accountd
//! (the planner discovers none), and [`decide_text`] is the language slot's route: a text-only
//! request is one stage. Running a plan is `porter-turns`'s `pipeline` module.

use crate::router::{Decided, Listed, fits, plain, slots_of};
use porter_core::capability::{LlmFeature, SpeechMode};
use porter_core::{Capability, DataClass, Need, Tier};
use porter_infer::{
    Answer, AutoPolicy, CatalogueModel, DescribeImages, Modality, PickCandidate, Pipeline,
    PlanRules, Policy, ProviderId, Refusal, RequestShape, RouteCandidate, Slot, TierMap, picks_for,
    plan_pipeline,
};
use std::collections::BTreeSet;

/// What a listed model takes in and gives out, from its capabilities.
fn modalities(listed: &Listed) -> (BTreeSet<Modality>, BTreeSet<Modality>) {
    let mut takes = BTreeSet::new();
    let mut gives = BTreeSet::new();
    for capability in &listed.card.capabilities {
        match capability {
            Capability::Llm(llm) => {
                takes.insert(Modality::Text);
                gives.insert(Modality::Text);
                if llm.features.contains(&LlmFeature::Vision) {
                    takes.insert(Modality::Image);
                }
                if llm.features.contains(&LlmFeature::AudioIn) {
                    takes.insert(Modality::Audio);
                }
            }
            Capability::ComputerUse(_) => {
                takes.extend([Modality::Text, Modality::Image]);
                gives.insert(Modality::Actions);
            }
            Capability::Embeddings(_) => {
                takes.insert(Modality::Text);
                gives.insert(Modality::Vector);
            }
            Capability::Speech(speech) => {
                if speech.modes.contains(&SpeechMode::Stt) {
                    takes.insert(Modality::Audio);
                    gives.insert(Modality::Text);
                }
                if speech.modes.contains(&SpeechMode::Tts) {
                    takes.insert(Modality::Text);
                    gives.insert(Modality::Audio);
                }
            }
            _ => {}
        }
    }
    (takes, gives)
}

/// The planner's catalogue: every listed model, in the slots its capabilities put it in. A model
/// is in the `text` slot only when it meets the session's language `need` (its features and
/// context), as routing has always required.
pub fn catalogue_of(listed: &[Listed], need: &Need) -> Vec<CatalogueModel> {
    listed
        .iter()
        .enumerate()
        .map(|(index, one)| {
            let (takes, gives) = modalities(one);
            let mut slots: BTreeSet<Slot> = slots_of(&one.card).into_iter().collect();
            if !fits(need, &one.card) {
                slots.remove(&Slot::Text);
            }
            CatalogueModel {
                candidate: PickCandidate {
                    route: RouteCandidate {
                        account: one.card.account.clone(),
                        model: one.card.model.clone(),
                        locality: one.card.locality.clone(),
                        billing: one.card.billing.clone(),
                        tier: porter_infer::TierChoice::Other,
                        permission: one.permission.clone(),
                        spend: one.spend,
                    },
                    readiness: one.readiness,
                    swap: one.swap.clone(),
                    licence: one.licence,
                    catalogue_index: u32::try_from(index).unwrap_or(u32::MAX),
                },
                takes,
                gives,
                slots,
                reach: one.provider.iter().cloned().collect(),
            }
        })
        .collect()
}

/// The providers the person's accounts reach: what accountd's `Verdicts` listed for the app. A
/// hosted model whose account says `ask` or `denied` is still listed, and `admit` refuses it by
/// its verdict (`NeedsGrant`, `Denied`), so the planner can say why instead of "nothing".
pub fn granted_of(listed: &[Listed]) -> Vec<ProviderId> {
    let mut providers: Vec<ProviderId> = listed
        .iter()
        .filter_map(|one| one.provider.clone())
        .collect();
    providers.sort();
    providers.dedup();
    providers
}

/// The pipeline for a request of `shape` under the person's settings and these models.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    shape: &RequestShape,
    need: &Need,
    tier: Tier,
    listed: &[Listed],
    policy: &Policy,
    tiers: &TierMap,
    auto: AutoPolicy,
    describe_images: DescribeImages,
) -> Result<Pipeline, Refusal> {
    plan_pipeline(
        shape,
        &picks_for(tiers, tier),
        &catalogue_of(listed, need),
        &granted_of(listed),
        PlanRules {
            policy,
            auto,
            describe_images,
        },
    )
}

/// The route of a language session: a text-only request, so a one-stage plan whose only stage is
/// the answer.
pub fn decide_text(
    need: &Need,
    class: DataClass,
    tier: Tier,
    listed: &[Listed],
    policy: &Policy,
    tiers: &TierMap,
    auto: AutoPolicy,
) -> Result<Decided, porter_infer::PickRefusal> {
    let shape = RequestShape {
        class,
        inputs: BTreeSet::from([Modality::Text]),
        answer: Answer::Text,
    };
    let pipeline = plan(
        &shape,
        need,
        tier,
        listed,
        policy,
        tiers,
        auto,
        DescribeImages::Off,
    )
    .map_err(|refusal| match refusal {
        Refusal::Stage { refusal, .. } => refusal,
        other => plain(other.infer_refusal()),
    })?;
    let stage = pipeline
        .stages
        .into_iter()
        .next()
        .ok_or(plain(porter_infer::InferRefusal::Unavailable))?;
    Ok(Decided {
        chosen: stage.picked.chosen,
        readiness: stage.picked.readiness,
        why: stage.picked.why,
    })
}
