//! Pipelines (capabilities.md section 3): from the shape of a request, the person's picks and the
//! catalogue, which models answer it, in which stages.
//!
//! [`plan_pipeline`] is pure and replayable. It takes the set of providers the person granted as
//! input and never discovers anything: a hosted model is a member of a slot only when one of its
//! reaches is in that set. Every stage is chosen by `pick` (or, for an empty row, by `route`),
//! so every answer comes out of `admit` first, against the class of what the stage receives:
//! a transcript carries the classes of the audio it came from, so composing stages can never move
//! voice data to a model its floor forbids.
//!
//! The stages, in order: `Hear` (audio to text, when the answering model cannot hear), `Describe`
//! (images to text, only when the person allowed it), `Answer`, `Speak` (text to audio, when the
//! answering model cannot speak). A named pick is never replaced: if it cannot serve, the plan is
//! a refusal that names it.

use crate::choice::ModelRef;
use crate::error::InferRefusal;
use crate::pick::{
    AutoPolicy, Declined, Pick, PickCandidate, PickPolicy, PickRefusal, Picked, Why,
};
use crate::policy::Policy;
use crate::readiness::Readiness;
use crate::reply::ServedBy;
use crate::route::{RouteAsk, route};
use crate::slot::Slot;
use porter_core::{DataClass, Locality, Tier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One kind of thing a model takes in or gives out (stoker's `Modality`, mirrored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// Text.
    Text,
    /// Images.
    Image,
    /// Audio.
    Audio,
    /// An embedding vector (output only).
    Vector,
    /// Computer-use actions (output only).
    Actions,
}

/// What the request wants back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Text.
    Text,
    /// Text, spoken.
    Speech,
}

/// What a request carries and wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestShape {
    /// The data class of everything on the request.
    pub class: DataClass,
    /// The modalities it carries.
    pub inputs: BTreeSet<Modality>,
    /// What it wants back.
    pub answer: Answer,
}

/// `ai.pipeline.describe_images`: whether an image the answering model cannot read may be
/// described by the `image_in` model first (the description loses detail, so it is shown).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescribeImages {
    /// Refuse, with the reason.
    #[default]
    Off,
    /// Describe first.
    On,
}

impl DescribeImages {
    /// The settings value.
    pub fn slug(self) -> &'static str {
        match self {
            DescribeImages::Off => "off",
            DescribeImages::On => "on",
        }
    }

    /// The value a settings string names.
    pub fn from_slug(slug: &str) -> Option<Self> {
        [DescribeImages::Off, DescribeImages::On]
            .into_iter()
            .find(|one| one.slug() == slug)
    }
}

/// A provider the person granted to inferd (`openrouter`, `anthropic`, ...): a provider file's id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(pub String);

/// The person's pick per slot, for one tier. A slot without an entry is an empty row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotPicks(pub BTreeMap<Slot, Pick>);

impl SlotPicks {
    /// The pick for `slot`, if the row says one.
    pub fn get(&self, slot: Slot) -> Option<&Pick> {
        self.0.get(&slot)
    }
}

/// One model the planner may use, with what it takes and gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueModel {
    /// What the hard rules and the pick read (its `route.tier` is set by the planner, per slot).
    pub candidate: PickCandidate,
    /// What it takes in (already narrowed by the engine that runs it).
    pub takes: BTreeSet<Modality>,
    /// What it gives out.
    pub gives: BTreeSet<Modality>,
    /// The slots whose signatures it satisfies.
    pub slots: BTreeSet<Slot>,
    /// The providers that reach it; empty for a model on this computer.
    pub reach: Vec<ProviderId>,
}

impl CatalogueModel {
    /// The model it is.
    pub fn model_ref(&self) -> ModelRef {
        self.candidate.model_ref()
    }

    fn reached_by(&self, granted: &[ProviderId]) -> bool {
        self.reach.is_empty() || self.reach.iter().any(|one| granted.contains(one))
    }
}

/// The rules every stage is held to.
#[derive(Debug, Clone, Copy)]
pub struct PlanRules<'a> {
    /// `ai.local_only` and the floors.
    pub policy: &'a Policy,
    /// `ai.auto.*`.
    pub auto: AutoPolicy,
    /// `ai.pipeline.describe_images`.
    pub describe_images: DescribeImages,
}

/// The data classes a stage receives: the joined label of its inputs. A stage is admitted against
/// the strictest floor among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassSet(pub BTreeSet<DataClass>);

impl ClassSet {
    /// The class whose floor is the tightest here (the first, in class order, among equals).
    /// `Public` for an empty set, which no stage has.
    pub fn strictest(&self, policy: &Policy) -> DataClass {
        self.0
            .iter()
            .copied()
            .min_by_key(|class| policy.floor(*class))
            .unwrap_or(DataClass::Public)
    }
}

/// What a stage does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageRole {
    /// Audio to text ("Heard by ...").
    Hear,
    /// Images to text ("Described by ...").
    Describe,
    /// The answer ("Answered by ...").
    Answer,
    /// Text to audio ("Spoken by ...").
    Speak,
}

/// One stage: the slot it was chosen from, the model, and why it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// What it does.
    pub role: StageRole,
    /// The slot whose pick chose it.
    pub slot: Slot,
    /// The model, its readiness and why it.
    pub picked: Picked,
    /// What it receives.
    pub receives: ClassSet,
    /// Where the model runs: what "sent to <provider>" says for this stage.
    pub locality: Locality,
}

impl Stage {
    /// Who runs the stage, as the wire names it.
    pub fn served(&self) -> ServedBy {
        ServedBy {
            account: self.picked.chosen.account.clone(),
            model: self.picked.chosen.model.clone(),
            locality: self.locality.clone(),
        }
    }
}

/// The stages that answer a request, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    /// At least the `Answer` stage.
    pub stages: Vec<Stage>,
}

/// Why no pipeline answers a request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// The answering model cannot read images and `ai.pipeline.describe_images` is off.
    ImagesNeedDescribing,
    /// A stage could not be served: the refusal, and the named model that could not serve.
    Stage {
        /// The stage.
        role: StageRole,
        /// What its pick resolved to.
        refusal: PickRefusal,
    },
    /// A shape the runner does not do yet.
    NotYet(&'static str),
}

/// What an empty row does: what `route` has always chosen among the models that can answer.
pub fn default_choice(
    ask: RouteAsk,
    candidates: &[PickCandidate],
    policy: &Policy,
) -> Result<Picked, PickRefusal> {
    let usable: Vec<&PickCandidate> = candidates
        .iter()
        .filter(|one| {
            !matches!(
                one.readiness,
                Readiness::Downloadable | Readiness::Unavailable
            )
        })
        .collect();
    let routes: Vec<_> = usable.iter().map(|one| one.route.clone()).collect();
    let chosen = route(ask, &routes, policy).map_err(PickRefusal::from)?;
    let readiness = usable
        .iter()
        .find(|one| one.route.account == chosen.account && one.route.model == chosen.model)
        .map_or(Readiness::Unavailable, |one| one.readiness);
    let why = if usable.len() == 1 {
        Why::OnlyOne
    } else {
        Why::CatalogueOrder
    };
    Ok(Picked {
        chosen,
        readiness,
        why,
    })
}

/// The model for one stage of `slot`: the person's pick over the slot's members, or the default
/// for an empty row.
fn choose_stage(
    slot: Slot,
    receives: &ClassSet,
    picks: &SlotPicks,
    members: &[&CatalogueModel],
    rules: PlanRules<'_>,
) -> Result<Picked, PickRefusal> {
    let pick_of = picks.get(slot);
    let candidates: Vec<PickCandidate> = members
        .iter()
        .enumerate()
        .map(|(index, one)| {
            let mut candidate = one.candidate.clone();
            candidate.route.tier = match pick_of {
                Some(Pick::Named(model)) if *model == one.model_ref() => {
                    crate::route::TierChoice::Chosen
                }
                _ => crate::route::TierChoice::Other,
            };
            candidate.catalogue_index = u32::try_from(index).unwrap_or(u32::MAX);
            candidate
        })
        .collect();
    let ask = RouteAsk {
        class: receives.strictest(rules.policy),
    };
    match pick_of {
        Some(chosen) => crate::pick::pick(
            ask,
            chosen,
            &candidates,
            PickPolicy {
                policy: rules.policy,
                auto: rules.auto,
            },
        ),
        None => default_choice(ask, &candidates, rules.policy),
    }
}

/// Where a picked model runs, from the catalogue it came from.
fn locality_of(picked: &Picked, catalogue: &[CatalogueModel]) -> Locality {
    catalogue
        .iter()
        .find(|one| {
            one.candidate.route.account == picked.chosen.account
                && one.candidate.route.model == picked.chosen.model
        })
        .map_or(Locality::Cloud { region: None }, |one| {
            one.candidate.route.locality.clone()
        })
}

/// The models that can serve `slot` for this person: in the slot, and reached by a granted
/// provider (or on this computer).
fn members<'a>(
    slot: Slot,
    catalogue: &'a [CatalogueModel],
    granted: &[ProviderId],
) -> Vec<&'a CatalogueModel> {
    catalogue
        .iter()
        .filter(|one| one.slots.contains(&slot) && one.reached_by(granted))
        .collect()
}

/// Whether the person named a model for `slot` that is not `model`: then `model` may not stand in
/// for the slot's stage.
fn names_another(picks: &SlotPicks, slot: Slot, model: &ModelRef) -> bool {
    matches!(picks.get(slot), Some(Pick::Named(named)) if named != model)
}

/// The pipeline for a request of this shape.
///
/// * One stage when the text slot's model takes every modality the request carries (audio goes
///   straight to a model that hears it), unless the person named a different model for the slot
///   that would convert it.
/// * Audio first through `voice_in`; images refused unless `describe_images` is on, then
///   `image_in` describes; the answer by the `text` slot; speech out through `voice_out`.
/// * The classes join: the text stage receives the audio's class (`Voice`) beside the request's.
///
/// `granted` is the providers the person granted to inferd; the planner discovers none.
pub fn plan_pipeline(
    shape: &RequestShape,
    picks: &SlotPicks,
    catalogue: &[CatalogueModel],
    granted: &[ProviderId],
    rules: PlanRules<'_>,
) -> Result<Pipeline, Refusal> {
    let hears = shape.inputs.contains(&Modality::Audio);
    let sees = shape.inputs.contains(&Modality::Image);
    let mut joined = BTreeSet::from([shape.class]);
    if hears {
        joined.insert(DataClass::Voice);
    }
    let all = ClassSet(joined);
    let stage = |role, slot, receives: &ClassSet| {
        choose_stage(
            slot,
            receives,
            picks,
            &members(slot, catalogue, granted),
            rules,
        )
        .map(|picked| Stage {
            role,
            slot,
            locality: locality_of(&picked, catalogue),
            picked,
            receives: receives.clone(),
        })
        .map_err(|refusal| Refusal::Stage { role, refusal })
    };
    let answer = stage(StageRole::Answer, Slot::Text, &all)?;
    let answering = catalogue
        .iter()
        .find(|one| {
            one.model_ref()
                == ModelRef {
                    account: answer.picked.chosen.account.clone(),
                    model: answer.picked.chosen.model.clone(),
                }
        })
        .ok_or(Refusal::NotYet("an answering model outside the catalogue"))?;
    let answering_ref = answering.model_ref();
    let takes = |modality| answering.takes.contains(&modality);
    let mut before = Vec::new();
    if hears && !(takes(Modality::Audio) && !names_another(picks, Slot::VoiceIn, &answering_ref)) {
        let audio = ClassSet(BTreeSet::from([DataClass::Voice, shape.class]));
        before.push(stage(StageRole::Hear, Slot::VoiceIn, &audio)?);
    }
    if sees && !takes(Modality::Image) {
        match rules.describe_images {
            DescribeImages::Off => return Err(Refusal::ImagesNeedDescribing),
            DescribeImages::On => {
                let image = ClassSet(BTreeSet::from([shape.class]));
                before.push(stage(StageRole::Describe, Slot::ImageIn, &image)?);
            }
        }
    }
    let speaks_itself = answering.gives.contains(&Modality::Audio)
        && !names_another(picks, Slot::VoiceOut, &answering_ref);
    let after = match shape.answer {
        Answer::Speech if !speaks_itself => vec![stage(StageRole::Speak, Slot::VoiceOut, &all)?],
        Answer::Speech | Answer::Text => Vec::new(),
    };
    Ok(Pipeline {
        stages: before.into_iter().chain([answer]).chain(after).collect(),
    })
}

/// A refusal that carries no model, for a caller that only needs the typed reason.
impl Refusal {
    /// The `InferRefusal` a session ends with for this.
    pub fn infer_refusal(&self) -> InferRefusal {
        match self {
            Refusal::ImagesNeedDescribing | Refusal::NotYet(_) => InferRefusal::Unsupported,
            Refusal::Stage { refusal, .. } => refusal.refusal,
        }
    }

    /// The named model that could not serve, when that is why.
    pub fn declined(&self) -> Option<&Declined> {
        match self {
            Refusal::Stage { refusal, .. } => refusal.declined.as_ref(),
            _ => None,
        }
    }
}

/// The tier a plan is for, spelled out where a caller builds [`SlotPicks`].
pub fn picks_for(map: &crate::choice::TierMap, tier: Tier) -> SlotPicks {
    SlotPicks(
        Slot::ALL
            .into_iter()
            .filter_map(|slot| Some((slot, map.pick(slot, tier)?)))
            .collect(),
    )
}

#[cfg(test)]
#[path = "pipeline/props.rs"]
mod props;
#[cfg(test)]
#[path = "pipeline/tests.rs"]
mod tests;
