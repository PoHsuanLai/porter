use super::*;
use crate::attached::config::{Place, Reach};
use crate::catalog::{CatalogDirs, local_claims, parse_entry_text, read_catalog};
use crate::entries;
use crate::testkit::Scratch;
use model_openai_compat::Flavor;
use porter_core::{Locality, ModelId, Subject};

fn attached(id: &str, place: Place) -> Attached {
    Attached {
        id: ModelId::parse(id).expect("id"),
        reach: Reach::Socket("/run/user/1000/lab.sock".into()),
        key_file: None,
        place,
        computer: None,
    }
}

fn catalogue() -> Vec<ModelEntry> {
    [entries::attached(), entries::chat(), entries::embed()]
        .iter()
        .map(|text| parse_entry_text(text).expect("entry"))
        .collect()
}

#[test]
fn an_attached_model_is_the_catalogues_entry_with_the_place_the_person_said() {
    let scratch = Scratch::new("am");
    let entries = catalogue();
    // The catalogue says on-device; the person's `where` is what routing sees.
    assert_eq!(entries[0].locality, model_catalog::Locality::OnDevice);
    let make = |place| {
        local_model(
            &attached(entries::ATTACHED, place),
            &entries,
            scratch.path(),
        )
        .expect("model")
    };
    let (here, there) = (make(Place::ThisDevice), make(Place::MyNetwork));
    assert_eq!(here.card.locality, Locality::OnDevice);
    assert_eq!(there.card.locality, Locality::LocalNetwork);
    for model in [&here, &there] {
        assert_eq!(model.card.billing, porter_core::Billing::Free);
        // A request names, and `/v1/models` must list, the name the catalogue says it is served as.
        assert_eq!(model.name.0, entries::SERVED);
        assert_eq!(model.entry.id.0, entries::ATTACHED);
        assert_eq!(model.card.model.as_str(), entries::ATTACHED);
        assert_eq!(model.spec.id.0, format!("attached:{}", entries::ATTACHED));
        // Never started: no GPU is claimed for it, and no weights are looked for.
        assert_eq!(model.spec.need, MiB(0));
        assert_eq!(model.weights(), crate::local::Weights::Present);
        assert_eq!(model.socket.0, PathBuf::from("/run/user/1000/lab.sock"));
        assert!(model.attached.is_some() && model.loopback.is_none());
        // The wire is the catalogue's `engine`.
        assert_eq!(model.flavor, Some(Flavor::Vllm));
        assert!(!model.card.capabilities.is_empty());
    }
}

#[test]
fn an_entry_inferd_launches_or_an_id_the_catalogue_lacks_is_a_typed_error() {
    let scratch = Scratch::new("am");
    let one = |id: &str| {
        local_model(
            &attached(id, Place::ThisDevice),
            &catalogue(),
            scratch.path(),
        )
    };
    assert_eq!(
        one("tiny-chat").err(),
        Some(AttachedError::NotAttachable {
            id: "tiny-chat".into()
        })
    );
    assert_eq!(
        one("not-there").err(),
        Some(AttachedError::NotInCatalogue {
            id: "not-there".into()
        })
    );
    let all = crate::attached::models(
        &[
            attached(entries::ATTACHED, Place::ThisDevice),
            attached("tiny-embed", Place::ThisDevice),
        ],
        &catalogue(),
        scratch.path(),
    );
    assert!(matches!(all, Err(AttachedError::NotAttachable { .. })));
}

#[test]
fn the_catalogue_alone_makes_no_claim_for_an_attached_entry_whatever_its_locality_says() {
    let shipped = CatalogDirs {
        system: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stoker/catalog"),
        user: PathBuf::from("/nonexistent/user/catalog"),
    };
    let catalog = read_catalog(&shipped);
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.id.0 == entries::ATTACHED)
        .expect("the shipped attached entry");
    assert!(
        entry.locality.is_on_device(),
        "the catalogue says on-device"
    );
    let claimed: Vec<_> = local_claims(&shipped)
        .into_iter()
        .filter_map(|claim| match claim.subject {
            Subject::Model(id) => Some(id),
            _ => None,
        })
        .collect();
    assert!(
        claimed.iter().all(|id| id.as_str() != entries::ATTACHED),
        "{claimed:?}"
    );
    assert!(!claimed.is_empty(), "the launched models still claim");
}
