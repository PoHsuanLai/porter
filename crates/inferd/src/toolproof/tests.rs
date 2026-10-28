use super::*;
use crate::testkit::Scratch;
use porter_capability::LlmFeature;
use porter_capability::ModelId;
use porter_core::AccountId;
use porter_core::{DataClass, Locality, Tier};
use porter_ids::{Count, Tokens};
use porter_match::LlmNeed;
use std::sync::Mutex as StdMutex;

fn model(name: &str) -> ModelRef {
    ModelRef {
        account: AccountId::parse("local").expect("account"),
        model: ModelId::parse(name).expect("model"),
    }
}

fn spec(features: &[LlmFeature]) -> SessionSpec {
    SessionSpec {
        need: Need::Llm(LlmNeed::new(
            features.iter().copied().collect(),
            Tokens(1000),
        )),
        class: DataClass::Public,
        tier: Tier::Balanced,
        usage: porter_core::consent::Usage::Interactive,
    }
}

fn served(name: &str) -> ServedBy {
    let model = model(name);
    ServedBy {
        account: model.account,
        model: model.model,
        locality: Locality::OnDevice,
    }
}

#[test]
fn what_is_found_is_kept_and_only_failures_are_listed() {
    let proofs = ToolProofs::in_memory();
    assert_eq!(proofs.get(&model("a")), None);
    proofs.record(model("a"), ToolProof::Passed);
    proofs.record(model("b"), ToolProof::Failed);
    assert_eq!(proofs.get(&model("a")), Some(ToolProof::Passed));
    assert_eq!(proofs.failed(), BTreeSet::from([model("b")]));
    // A later finding replaces the earlier one, and the cache is by model: the same name on
    // another account is another model.
    proofs.record(model("a"), ToolProof::Failed);
    assert_eq!(proofs.failed(), BTreeSet::from([model("a"), model("b")]));
    let other = ModelRef {
        account: AccountId::parse("cloud").expect("account"),
        ..model("a")
    };
    assert_eq!(proofs.get(&other), None);
}

#[test]
fn the_cache_survives_a_restart_and_an_unreadable_file_is_an_empty_cache() {
    let scratch = Scratch::new("tool-proofs");
    let file = scratch.path().join("sub").join("tool-proofs.json");
    ToolProofs::open(file.clone()).record(model("a"), ToolProof::Failed);
    assert_eq!(
        ToolProofs::open(file.clone()).get(&model("a")),
        Some(ToolProof::Failed)
    );
    std::fs::write(&file, "not json").expect("write");
    assert_eq!(ToolProofs::open(file).failed(), BTreeSet::new());
}

#[derive(Debug, Default)]
struct Seen(StdMutex<usize>);

impl AuditSink for Seen {
    fn record(&self, _: &SessionSpec, _: &ServedBy, _: &InferReply, _: &Carried) {
        *self.0.lock().expect("lock") += 1;
    }
}

#[test]
fn a_turn_that_offered_tools_and_ended_unreadable_marks_the_model_and_is_still_recorded() {
    let carried = Carried {
        images: Count(0),
        audio_ms: Count(0),
    };
    let tools = [LlmFeature::Chat, LlmFeature::Tools];
    let cases = [
        (
            "tools, unreadable",
            &tools[..],
            InferReply::Failed(ModelError::Unparseable),
            true,
        ),
        (
            "no tools, unreadable",
            &tools[..1],
            InferReply::Failed(ModelError::Unparseable),
            false,
        ),
        ("tools, cancelled", &tools[..], InferReply::Cancelled, false),
        (
            "tools, unreachable",
            &tools[..],
            InferReply::Failed(ModelError::Unreachable),
            false,
        ),
    ];
    for (name, features, reply, marks) in cases {
        let proofs = ToolProofs::in_memory();
        let watch = ToolWatch::new(Seen::default(), proofs.clone());
        watch.record(&spec(features), &served("a"), &reply, &carried);
        assert_eq!(proofs.failed().contains(&model("a")), marks, "{name}");
        assert_eq!(*watch.inner.0.lock().expect("lock"), 1, "{name}: recorded");
    }
}
