use super::tests::{hearing_text, hosted, kokoro, local, rules, set, shape, text_model, whisper};
use super::*;
use crate::policy::{ClassFloor, Floor, LocalOnly};
use proptest::prelude::*;

fn class() -> impl Strategy<Value = DataClass> {
    prop_oneof![
        Just(DataClass::Public),
        Just(DataClass::Notes),
        Just(DataClass::Mail),
        Just(DataClass::Prompt),
    ]
}

fn inputs() -> impl Strategy<Value = Vec<Modality>> {
    proptest::sample::subsequence(
        vec![Modality::Text, Modality::Audio, Modality::Image],
        1..=3,
    )
}

fn catalogue() -> impl Strategy<Value = Vec<CatalogueModel>> {
    proptest::sample::subsequence(
        vec![
            text_model("t1"),
            text_model("t2"),
            hearing_text("hear"),
            whisper(),
            kokoro(),
            local(
                "see",
                set([Modality::Image]),
                set([Modality::Text]),
                set([Slot::ImageIn]),
            ),
            hosted("cloud-a", &["anthropic"], set([Slot::Text, Slot::ImageIn])),
            hosted("cloud-b", &["openrouter", "openai"], set([Slot::Text])),
        ],
        0..=8,
    )
}

fn picks() -> impl Strategy<Value = SlotPicks> {
    let names = [
        "t1", "t2", "hear", "whisper", "kokoro", "see", "cloud-a", "cloud-b", "ghost",
    ];
    let one = (
        proptest::sample::select(vec![
            Slot::Text,
            Slot::VoiceIn,
            Slot::VoiceOut,
            Slot::ImageIn,
        ]),
        proptest::option::of(prop_oneof![
            Just(None),
            proptest::sample::select(names.to_vec()).prop_map(Some)
        ]),
    );
    proptest::collection::vec(one, 0..4).prop_map(|rows| {
        SlotPicks(
            rows.into_iter()
                .filter_map(|(slot, named)| {
                    let pick = match named? {
                        Some(name) => Pick::Named(super::tests::mref(name)),
                        None => Pick::Auto(Default::default()),
                    };
                    Some((slot, pick))
                })
                .collect(),
        )
    })
}

fn granted() -> impl Strategy<Value = Vec<ProviderId>> {
    proptest::sample::subsequence(vec!["anthropic", "openrouter", "openai"], 0..=3).prop_map(
        |names| {
            names
                .into_iter()
                .map(|n| ProviderId(n.to_owned()))
                .collect()
        },
    )
}

fn policy() -> impl Strategy<Value = Policy> {
    (
        any::<bool>(),
        proptest::sample::select(vec![Floor::OnDevice, Floor::Anywhere]),
    )
        .prop_map(|(local_only, voice)| Policy {
            local_only: if local_only {
                LocalOnly::On
            } else {
                LocalOnly::Off
            },
            floors: vec![
                ClassFloor {
                    class: DataClass::Voice,
                    floor: voice,
                },
                ClassFloor {
                    class: DataClass::Mail,
                    floor: Floor::OnDevice,
                },
            ],
        })
}

proptest! {
    /// No stage of any plan is a model `admit` would refuse for what the stage receives, a
    /// hosted model is only used with a granted provider, and the same inputs plan the same way.
    #[test]
    fn plans_never_violate_admit_and_are_deterministic(
        class in class(),
        inputs in inputs(),
        speech in any::<bool>(),
        catalogue in catalogue(),
        picks in picks(),
        granted in granted(),
        policy in policy(),
        describe in any::<bool>(),
    ) {
        let shape = shape(class, &inputs, if speech { Answer::Speech } else { Answer::Text });
        let describe = if describe { DescribeImages::On } else { DescribeImages::Off };
        let once = plan_pipeline(&shape, &picks, &catalogue, &granted, rules(&policy, describe));
        let again = plan_pipeline(&shape, &picks, &catalogue, &granted, rules(&policy, describe));
        prop_assert_eq!(&once, &again);
        if let Ok(pipeline) = once {
            for stage in &pipeline.stages {
                let listed = catalogue.iter().find(|one| {
                    one.candidate.route.model == stage.picked.chosen.model
                }).expect("listed");
                for class in &stage.receives.0 {
                    prop_assert!(
                        admit(RouteAsk { class: *class }, [&listed.candidate.route], &policy).is_ok(),
                        "{:?} receives {:?}", stage.role, class
                    );
                }
                prop_assert!(listed.reach.is_empty()
                    || listed.reach.iter().any(|one| granted.contains(one)));
                prop_assert!(listed.slots.contains(&stage.slot));
            }
            prop_assert_eq!(pipeline.stages.iter().filter(|s| s.role == StageRole::Answer).count(), 1);
        }
    }

    /// A named pick is never replaced: when a plan uses the slot, the stage runs the named model.
    #[test]
    fn a_named_pick_is_never_replaced(
        class in class(),
        inputs in inputs(),
        speech in any::<bool>(),
        catalogue in catalogue(),
        picks in picks(),
        granted in granted(),
        policy in policy(),
    ) {
        let shape = shape(class, &inputs, if speech { Answer::Speech } else { Answer::Text });
        let got = plan_pipeline(&shape, &picks, &catalogue, &granted, rules(&policy, DescribeImages::On));
        if let Ok(pipeline) = got {
            for stage in &pipeline.stages {
                if let Some(Pick::Named(named)) = picks.get(stage.slot) {
                    prop_assert_eq!(&named.model, &stage.picked.chosen.model);
                }
            }
        }
    }
}

use crate::route::admit;
