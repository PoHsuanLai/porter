use super::*;
use crate::testkit::Scratch;
use crate::testkit::entries;
use porter_core::capability::{CuaBatching, Offered};
use std::path::PathBuf;

fn shipped() -> CatalogDirs {
    CatalogDirs {
        system: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../stoker/catalog"),
        user: PathBuf::from("/nonexistent/user/catalog"),
    }
}

fn capabilities(id: &str) -> Vec<Capability> {
    let catalog = read_catalog(&shipped());
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.id.0 == id)
        .unwrap_or_else(|| panic!("no shipped entry {id}"));
    capabilities_of(entry)
}

#[test]
fn the_shipped_catalog_reads_without_a_skipped_file() {
    let catalog = read_catalog(&shipped());
    assert_eq!(catalog.skipped, vec![]);
    let mut ids: Vec<&str> = catalog.entries.iter().map(|e| e.id.0.as_str()).collect();
    ids.sort_unstable();
    assert!(ids.contains(&"holo-3.1-4b"), "{ids:?}");
    assert!(ids.contains(&"kokoro-82m"), "{ids:?}");
}

#[test]
fn holo_is_a_language_model_and_a_computer_use_model() {
    let caps = capabilities("holo-3.1-4b");
    let Some(Capability::Llm(llm)) = caps.iter().find(|c| matches!(c, Capability::Llm(_))) else {
        panic!("a language model: {caps:?}");
    };
    assert_eq!(llm.context, Tokens(32768));
    assert_eq!(llm.max_output, Tokens(4096));
    assert_eq!(llm.wire, LlmWire::ChatCompletions);
    assert_eq!(
        llm.features,
        [
            LlmFeature::Chat,
            LlmFeature::Tools,
            LlmFeature::Vision,
            LlmFeature::StructuredOutput,
            LlmFeature::Reasoning,
        ]
        .into()
    );
    let Some(Capability::ComputerUse(cua)) = caps
        .iter()
        .find(|c| matches!(c, Capability::ComputerUse(_)))
    else {
        panic!("computer use: {caps:?}");
    };
    assert_eq!(cua.environments, BTreeSet::from([CuaEnv::Desktop]));
    assert_eq!(cua.batching, CuaBatching::One);
    assert_eq!(cua.zoom, Offered::Absent);
    // The longest side the entry's pixel cap allows: the side of a square of 16777216 pixels.
    assert_eq!(cua.max_image, Px(4096));
}

#[test]
fn speech_models_make_speech_claims_in_their_direction() {
    let kind = |id: &str| match capabilities(id).as_slice() {
        [Capability::Speech(cap)] => cap.clone(),
        other => panic!("one speech claim for {id}: {other:?}"),
    };
    let whisper = kind("whisper-large-v3");
    assert_eq!(whisper.modes, BTreeSet::from([SpeechMode::Stt]));
    assert_eq!(whisper.languages, LanguageSet::Any);
    let kokoro = kind("kokoro-82m");
    assert_eq!(kokoro.modes, BTreeSet::from([SpeechMode::Tts]));
    let LanguageSet::Listed(langs) = kokoro.languages else {
        panic!("listed languages");
    };
    assert_eq!(langs.len(), 4);
}

#[test]
fn local_claims_are_curated_per_model_and_leave_embedding_models_out() {
    let claims = local_claims(&shipped());
    assert!(!claims.is_empty());
    assert!(claims.iter().all(|c| c.provenance == Provenance::Curated));
    assert!(
        claims
            .iter()
            .all(|c| matches!(c.subject, Subject::Model(_)) && matches!(c.offer, Offer::Present(_)))
    );
    assert!(
        claims
            .iter()
            .all(|c| !matches!(&c.offer, Offer::Present(Capability::Embeddings(_)))),
        "the shipped catalog has no embedding entry"
    );
}

#[test]
fn an_embedding_entry_makes_a_claim_from_its_embed_table() {
    let entry = parse_entry(&entries::embed()).expect("entry");
    let claims = claims_of(&entry);
    let [
        Claim {
            offer: Offer::Present(Capability::Embeddings(cap)),
            subject: Subject::Model(model),
            provenance: Provenance::Curated,
        },
    ] = claims.as_slice()
    else {
        panic!("one embeddings claim: {claims:?}");
    };
    assert_eq!(model.as_str(), "tiny-embed");
    assert_eq!(cap.dims, Dims(4));
    assert_eq!(cap.max_input, Tokens(512));
    assert_eq!(cap.max_batch, Count(2));
    assert_eq!(cap.prompts.query, PrefixText("search_query: ".into()));
    assert_eq!(cap.modalities, BTreeSet::from([Modality::Text]));
}

#[test]
fn a_user_file_replaces_the_system_file_and_a_bad_file_is_skipped_and_named() {
    let (system, user) = (Scratch::new("system"), Scratch::new("user"));
    std::fs::write(system.path().join("tiny-chat.toml"), entries::chat()).expect("write");
    std::fs::write(
        user.path().join("tiny-chat.toml"),
        entries::chat().replace("context = 8192", "context = 4096"),
    )
    .expect("write");
    std::fs::write(user.path().join("broken.toml"), "id = ").expect("write");
    std::fs::write(user.path().join("notes.txt"), "not a catalog file").expect("write");
    let catalog = read_catalog(&CatalogDirs {
        system: system.path().to_path_buf(),
        user: user.path().to_path_buf(),
    });
    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(
        catalog.entries[0].caps.as_ref().map(|caps| caps.context.0),
        Some(4096)
    );
    assert_eq!(catalog.skipped.len(), 1);
    assert!(catalog.skipped[0].what.ends_with("broken.toml"));
}

#[test]
fn a_missing_directory_is_an_empty_catalog() {
    let catalog = read_catalog(&CatalogDirs {
        system: PathBuf::from("/nonexistent/a"),
        user: PathBuf::from("/nonexistent/b"),
    });
    assert_eq!(catalog, Catalog::default());
}

#[test]
fn a_computer_use_entry_claims_for_the_dialects_an_engine_here_can_speak() {
    let claims = |dialect: &str| {
        let text = entries::cua().replace(r#"{ kind = "tool", v = "holo31" }"#, dialect);
        capabilities_of(&parse_entry(&text).expect("entry"))
            .iter()
            .any(|c| matches!(c, Capability::ComputerUse(_)))
    };
    assert!(claims(r#"{ kind = "tool", v = "holo31" }"#));
    assert!(claims(r#"{ kind = "text", v = "ui_tars15" }"#));
    assert!(
        !claims(r#"{ kind = "wire", v = "open_ai_computer" }"#),
        "a vendor's wire needs the vendor's backend"
    );
    let text = entries::cua().replace(
        r#"{ kind = "tool", v = "holo31" }"#,
        r#"{ kind = "wire", v = "open_ai_computer" }"#,
    );
    let caps = capabilities_of(&parse_entry(&text).expect("entry"));
    assert!(caps.iter().any(|c| matches!(c, Capability::Llm(_))));
}
