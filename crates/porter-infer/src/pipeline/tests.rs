use super::*;
use crate::choice::LicenceClass;
use crate::pick::SwapCost;
use crate::policy::{ClassFloor, Floor, LocalOnly};
use crate::route::{RouteCandidate, TierChoice};
use crate::spend::SpendVerdict;
use porter_core::consent::{GrantScope, Verdict};
use porter_core::{AccountId, Billing, GrantId, Locality, ModelId};

pub(super) fn mref(name: &str) -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse(name).expect("id"),
    }
}

pub(super) fn set<T: Ord, const N: usize>(items: [T; N]) -> BTreeSet<T> {
    items.into_iter().collect()
}

/// A local model that takes and gives these, listed in these slots.
pub(super) fn local(
    name: &str,
    takes: BTreeSet<Modality>,
    gives: BTreeSet<Modality>,
    slots: BTreeSet<Slot>,
) -> CatalogueModel {
    let model = mref(name);
    CatalogueModel {
        candidate: PickCandidate {
            route: RouteCandidate {
                account: model.account,
                model: model.model,
                locality: Locality::OnDevice,
                billing: Billing::Free,
                tier: TierChoice::Other,
                permission: Verdict::Granted {
                    grant: GrantId::parse("g").expect("id"),
                    scope: GrantScope::Always,
                },
                spend: SpendVerdict::Within,
            },
            readiness: Readiness::Ready,
            swap: SwapCost::Resident,
            licence: LicenceClass::Open,
            catalogue_index: 0,
        },
        takes,
        gives,
        slots,
        reach: Vec::new(),
    }
}

/// A hosted model reached through `providers`.
pub(super) fn hosted(name: &str, providers: &[&str], slots: BTreeSet<Slot>) -> CatalogueModel {
    let mut model = local(
        name,
        set([Modality::Text, Modality::Image]),
        set([Modality::Text]),
        slots,
    );
    model.candidate.route.locality = Locality::Cloud { region: None };
    model.reach = providers
        .iter()
        .map(|one| ProviderId((*one).to_owned()))
        .collect();
    model
}

pub(super) fn text_model(name: &str) -> CatalogueModel {
    local(
        name,
        set([Modality::Text]),
        set([Modality::Text]),
        set([Slot::Text]),
    )
}

pub(super) fn whisper() -> CatalogueModel {
    local(
        "whisper",
        set([Modality::Audio]),
        set([Modality::Text]),
        set([Slot::VoiceIn]),
    )
}

pub(super) fn kokoro() -> CatalogueModel {
    local(
        "kokoro",
        set([Modality::Text]),
        set([Modality::Audio]),
        set([Slot::VoiceOut]),
    )
}

pub(super) fn holo() -> CatalogueModel {
    local(
        "holo",
        set([Modality::Text, Modality::Image]),
        set([Modality::Text, Modality::Actions]),
        set([Slot::Text, Slot::ImageIn, Slot::ComputerUse]),
    )
}

pub(super) fn hearing_text(name: &str) -> CatalogueModel {
    local(
        name,
        set([Modality::Text, Modality::Image, Modality::Audio]),
        set([Modality::Text]),
        set([Slot::Text, Slot::VoiceIn, Slot::ImageIn]),
    )
}

pub(super) fn shape(class: DataClass, inputs: &[Modality], answer: Answer) -> RequestShape {
    RequestShape {
        class,
        inputs: inputs.iter().copied().collect(),
        answer,
    }
}

pub(super) fn rules(policy: &Policy, describe: DescribeImages) -> PlanRules<'_> {
    PlanRules {
        policy,
        auto: AutoPolicy::default(),
        describe_images: describe,
    }
}

pub(super) fn open() -> Policy {
    Policy {
        local_only: LocalOnly::Off,
        floors: vec![ClassFloor {
            class: DataClass::Voice,
            floor: Floor::OnDevice,
        }],
    }
}

fn plan(
    shape: &RequestShape,
    picks: &SlotPicks,
    catalogue: &[CatalogueModel],
    granted: &[ProviderId],
    describe: DescribeImages,
) -> Result<Vec<(StageRole, ModelRef)>, Refusal> {
    let policy = open();
    plan_pipeline(shape, picks, catalogue, granted, rules(&policy, describe)).map(|pipeline| {
        pipeline
            .stages
            .into_iter()
            .map(|stage| {
                (
                    stage.role,
                    ModelRef {
                        account: stage.picked.chosen.account,
                        model: stage.picked.chosen.model,
                    },
                )
            })
            .collect()
    })
}

fn named(slot: Slot, name: &str) -> SlotPicks {
    SlotPicks(BTreeMap::from([(slot, Pick::Named(mref(name)))]))
}

const T: Modality = Modality::Text;
const A: Modality = Modality::Audio;
const I: Modality = Modality::Image;

#[test]
fn text_alone_is_one_stage() {
    let catalogue = [text_model("gemma"), whisper()];
    let got = plan(
        &shape(DataClass::Notes, &[T], Answer::Text),
        &SlotPicks::default(),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(got, Ok(vec![(StageRole::Answer, mref("gemma"))]));
}

#[test]
fn audio_goes_through_voice_in_when_the_text_model_cannot_hear() {
    let catalogue = [text_model("gemma"), whisper()];
    let got = plan(
        &shape(DataClass::Prompt, &[A], Answer::Text),
        &SlotPicks::default(),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(
        got,
        Ok(vec![
            (StageRole::Hear, mref("whisper")),
            (StageRole::Answer, mref("gemma"))
        ])
    );
}

#[test]
fn a_text_model_that_hears_is_one_stage_unless_another_hearer_is_named() {
    let catalogue = [hearing_text("gemma-e4b"), whisper()];
    let shape = shape(DataClass::Prompt, &[A], Answer::Text);
    let auto = plan(
        &shape,
        &SlotPicks::default(),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(auto, Ok(vec![(StageRole::Answer, mref("gemma-e4b"))]));
    let same = plan(
        &shape,
        &named(Slot::VoiceIn, "gemma-e4b"),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(same, auto);
    let other = plan(
        &shape,
        &named(Slot::VoiceIn, "whisper"),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(
        other,
        Ok(vec![
            (StageRole::Hear, mref("whisper")),
            (StageRole::Answer, mref("gemma-e4b"))
        ])
    );
}

#[test]
fn images_are_refused_unless_describing_is_on() {
    let catalogue = [text_model("gemma"), holo()];
    let shape = shape(DataClass::Notes, &[T, I], Answer::Text);
    let picks = named(Slot::Text, "gemma");
    assert_eq!(
        plan(&shape, &picks, &catalogue, &[], DescribeImages::Off),
        Err(Refusal::ImagesNeedDescribing)
    );
    assert_eq!(
        plan(&shape, &picks, &catalogue, &[], DescribeImages::On),
        Ok(vec![
            (StageRole::Describe, mref("holo")),
            (StageRole::Answer, mref("gemma"))
        ])
    );
}

#[test]
fn a_text_model_that_reads_images_needs_no_describer() {
    let catalogue = [holo(), text_model("gemma")];
    let got = plan(
        &shape(DataClass::Notes, &[T, I], Answer::Text),
        &named(Slot::Text, "holo"),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(got, Ok(vec![(StageRole::Answer, mref("holo"))]));
}

#[test]
fn speech_out_goes_through_voice_out() {
    let catalogue = [text_model("gemma"), kokoro()];
    let got = plan(
        &shape(DataClass::Notes, &[T], Answer::Speech),
        &SlotPicks::default(),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    assert_eq!(
        got,
        Ok(vec![
            (StageRole::Answer, mref("gemma")),
            (StageRole::Speak, mref("kokoro"))
        ])
    );
}

#[test]
fn all_four_stages_in_order() {
    let catalogue = [text_model("gemma"), whisper(), kokoro(), holo()];
    let got = plan(
        &shape(DataClass::Prompt, &[A, I], Answer::Speech),
        &named(Slot::Text, "gemma"),
        &catalogue,
        &[],
        DescribeImages::On,
    );
    let roles: Vec<StageRole> = got.expect("plan").into_iter().map(|(r, _)| r).collect();
    assert_eq!(
        roles,
        [
            StageRole::Hear,
            StageRole::Describe,
            StageRole::Answer,
            StageRole::Speak
        ]
    );
}

#[test]
fn a_named_pick_that_cannot_serve_refuses_and_is_not_replaced() {
    // The person named the cloud model for text; nothing is granted, so it is not listed.
    let catalogue = [
        hosted("claude", &["anthropic"], set([Slot::Text])),
        text_model("gemma"),
    ];
    let got = plan(
        &shape(DataClass::Public, &[T], Answer::Text),
        &named(Slot::Text, "claude"),
        &catalogue,
        &[],
        DescribeImages::Off,
    );
    match got {
        Err(Refusal::Stage { role, refusal }) => {
            assert_eq!(role, StageRole::Answer);
            assert_eq!(refusal.declined.map(|d| d.model), Some(mref("claude")));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_hosted_model_is_a_candidate_only_with_a_granted_provider() {
    let catalogue = [hosted(
        "claude",
        &["anthropic", "openrouter"],
        set([Slot::Text]),
    )];
    let shape = shape(DataClass::Public, &[T], Answer::Text);
    let cases = [
        (vec![], false),
        (vec![ProviderId("openai".into())], false),
        (vec![ProviderId("openrouter".into())], true),
        (vec![ProviderId("anthropic".into())], true),
    ];
    for (granted, serves) in cases {
        let got = plan(
            &shape,
            &SlotPicks::default(),
            &catalogue,
            &granted,
            DescribeImages::Off,
        );
        assert_eq!(got.is_ok(), serves, "{granted:?}");
    }
}

#[test]
fn voice_never_reaches_a_cloud_transcriber_under_its_floor() {
    let mut cloud_stt = hosted("cloud-stt", &["openai"], set([Slot::VoiceIn]));
    cloud_stt.takes = set([A]);
    let catalogue = [text_model("gemma"), cloud_stt];
    let got = plan(
        &shape(DataClass::Public, &[A], Answer::Text),
        &SlotPicks::default(),
        &catalogue,
        &[ProviderId("openai".into())],
        DescribeImages::Off,
    );
    match got {
        Err(Refusal::Stage { role, refusal }) => {
            assert_eq!(role, StageRole::Hear);
            assert_eq!(
                refusal.refusal,
                InferRefusal::RequiresCloud(DataClass::Voice)
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_transcript_keeps_the_voice_floor_for_the_text_stage() {
    // A cloud text model, a local hearer: the request is Public, but the words came from voice.
    let catalogue = [
        hosted("claude", &["anthropic"], set([Slot::Text])),
        whisper(),
    ];
    let got = plan(
        &shape(DataClass::Public, &[A], Answer::Text),
        &SlotPicks::default(),
        &catalogue,
        &[ProviderId("anthropic".into())],
        DescribeImages::Off,
    );
    match got {
        Err(Refusal::Stage { role, refusal }) => {
            assert_eq!(role, StageRole::Answer);
            assert_eq!(
                refusal.refusal,
                InferRefusal::RequiresCloud(DataClass::Voice)
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn auto_follows_pick_and_names_a_reason() {
    let catalogue = [text_model("a"), text_model("b")];
    let policy = open();
    let picks = SlotPicks(BTreeMap::from([(
        Slot::Text,
        Pick::Auto(Default::default()),
    )]));
    let pipeline = plan_pipeline(
        &shape(DataClass::Notes, &[T], Answer::Text),
        &picks,
        &catalogue,
        &[],
        rules(&policy, DescribeImages::Off),
    )
    .expect("plan");
    assert_eq!(pipeline.stages[0].picked.why, Why::CatalogueOrder);
}

#[test]
fn describe_images_slugs_round_trip() {
    for one in [DescribeImages::Off, DescribeImages::On] {
        assert_eq!(DescribeImages::from_slug(one.slug()), Some(one));
    }
    assert_eq!(DescribeImages::default(), DescribeImages::Off);
}
