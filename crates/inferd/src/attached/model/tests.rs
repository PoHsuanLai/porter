use super::*;
use crate::attached::config::{Place, Reach};
use crate::catalog::parse_entry_text;
use crate::entries;
use crate::testkit::Scratch;
use model_openai_compat::Flavor;
use porter_core::{Locality, ModelId};

fn attached(id: &str, place: Place) -> Attached {
    Attached {
        id: ModelId::parse(id).expect("id"),
        reach: Reach::Socket("/run/user/1000/lab.sock".into()),
        key_file: None,
        place,
    }
}

fn catalogue() -> Vec<ModelEntry> {
    [entries::chat(), entries::embed(), entries::cua()]
        .iter()
        .map(|text| parse_entry_text(text).expect("entry"))
        .collect()
}

#[test]
fn an_attached_model_is_the_catalogues_entry_with_the_place_the_person_said() {
    let scratch = Scratch::new("am");
    let here = local_model(
        &attached("tiny-chat", Place::ThisDevice),
        &catalogue(),
        scratch.path(),
    )
    .expect("model");
    let there = local_model(
        &attached("tiny-chat", Place::MyNetwork),
        &catalogue(),
        scratch.path(),
    )
    .expect("model");
    assert_eq!(here.card.locality, Locality::OnDevice);
    assert_eq!(there.card.locality, Locality::LocalNetwork);
    for model in [&here, &there] {
        assert_eq!(model.card.billing, porter_core::Billing::Free);
        assert_eq!(model.name.0, "tiny-chat");
        assert_eq!(model.entry.id.0, "tiny-chat");
        assert_eq!(model.spec.id.0, "attached:tiny-chat");
        // Never started: no GPU is claimed for it, and no weights are looked for.
        assert_eq!(model.spec.need, MiB(0));
        assert_eq!(model.weights(), crate::local::Weights::Present);
        assert_eq!(model.socket.0, PathBuf::from("/run/user/1000/lab.sock"));
        assert!(model.attached.is_some() && model.loopback.is_none());
        assert!(model.flavor.is_some());
    }
    assert_eq!(here.flavor, Some(Flavor::LlamaServer));
    // The same card as the catalogue makes for that id, apart from where it runs.
    let built = crate::local::build(
        &catalogue(),
        &crate::local::EngineConfig {
            llama_server: Some("/nonexistent/llama-server".into()),
            vllm_python: Some("/nonexistent/python".into()),
            ..crate::local::EngineConfig::default()
        },
        scratch.path(),
    );
    let same = built
        .iter()
        .find(|model| model.entry.id.0 == "tiny-chat")
        .expect("built");
    assert_eq!(here.card.capabilities, same.card.capabilities);
}

#[test]
fn an_id_the_catalogue_lacks_or_an_entry_with_no_chat_engine_is_a_typed_error() {
    let scratch = Scratch::new("am");
    assert_eq!(
        local_model(
            &attached("not-there", Place::ThisDevice),
            &catalogue(),
            scratch.path()
        )
        .err(),
        Some(AttachedError::NotInCatalogue {
            id: "not-there".into()
        })
    );
    let all = crate::attached::models(
        &[
            attached("tiny-chat", Place::ThisDevice),
            attached("nope", Place::ThisDevice),
        ],
        &catalogue(),
        scratch.path(),
    );
    assert_eq!(
        all.err(),
        Some(AttachedError::NotInCatalogue { id: "nope".into() })
    );
}
