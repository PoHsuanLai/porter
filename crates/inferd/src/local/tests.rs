use super::*;
use crate::catalog::parse_entry_text;
use crate::entries;
use crate::testkit::{Scratch, models};
use porter_core::capability::LlmFeature;

#[test]
fn each_entry_with_a_capability_and_a_configured_engine_becomes_a_model() {
    let scratch = Scratch::new("local");
    let models = models(&scratch);
    let ids: Vec<&str> = models.iter().map(|m| m.card.model.as_str()).collect();
    assert_eq!(ids, ["tiny-chat", "tiny-embed", "tiny-cua"]);
    for model in &models {
        assert_eq!(model.card.account.as_str(), LOCAL_ACCOUNT);
        assert_eq!(model.card.locality, Locality::OnDevice);
        assert_eq!(model.card.billing, Billing::Free);
        assert_eq!(model.name.0, model.entry.id.0);
    }
    let chat = &models[0];
    assert_eq!(chat.flavor, Some(Flavor::LlamaServer));
    assert_eq!(chat.spec.id.0, "llama_server:tiny-chat");
    // weights + kv of 8192 tokens (1 MiB per 1k, rounded up) + overhead.
    assert_eq!(chat.spec.need.0, 100 + 9 + 50);
    assert!(chat.socket.0.ends_with("llama_server-tiny-chat.sock"));
    assert_eq!(models[2].flavor, Some(Flavor::Vllm));
    assert_eq!(models[2].spec.id.0, "vllm:tiny-cua");
}

#[test]
fn an_embedding_model_carries_its_catalog_table_and_without_one_it_is_not_a_model() {
    let scratch = Scratch::new("local-embed");
    let models = models(&scratch);
    assert_eq!(
        models[1].embed().map(|embed| embed.dims.0),
        Some(4),
        "read from the entry's `embed` table"
    );
    assert_eq!(
        models[1].caps(),
        None,
        "an embeddings-only entry has no chat fields"
    );
    // The cache of an embedding-only entry is sized for its longest input (512 tokens).
    assert_eq!(models[1].spec.need.0, 50 + 1 + 50);
    let config = EngineConfig {
        llama_server: Some(PathBuf::from("/x")),
        hf_cache: scratch.path().join("hf"),
        ..EngineConfig::default()
    };
    // The same entry without its table, kept as an embeddings role with chat fields, makes no
    // claim and so no model.
    let chat_fields = parse_entry_text(
        &entries::chat().replace(r#"roles = ["llm"]"#, r#"roles = ["embeddings"]"#),
    )
    .expect("entry");
    assert!(chat_fields.embed.is_none());
    assert!(build(&[chat_fields], &config, scratch.path()).is_empty());
}

#[test]
fn an_engine_kind_with_no_program_is_not_offered() {
    let scratch = Scratch::new("local-programs");
    let entries: Vec<_> = [entries::chat(), entries::cua()]
        .iter()
        .map(|text| parse_entry_text(text).expect("entry"))
        .collect();
    // Only vLLM is configured: the llama-server model is left out.
    let config = EngineConfig {
        vllm_python: Some(PathBuf::from("/py")),
        hf_cache: scratch.path().join("hf"),
        ..EngineConfig::default()
    };
    let only_vllm = build(&entries, &config, scratch.path());
    let ids: Vec<&str> = only_vllm.iter().map(|m| m.card.model.as_str()).collect();
    assert_eq!(ids, ["tiny-cua"]);
    assert!(build(&entries, &EngineConfig::default(), scratch.path()).is_empty());
}

#[test]
fn weights_are_looked_for_live_in_the_cache() {
    let scratch = Scratch::new("local-weights");
    let entries = [parse_entry_text(&entries::chat()).expect("entry")];
    let config = EngineConfig {
        llama_server: Some(PathBuf::from("/x")),
        hf_cache: scratch.path().join("hf"),
        ..EngineConfig::default()
    };
    let models = build(&entries, &config, scratch.path());
    let model = &models[0];
    assert_eq!(model.weights(), Weights::Missing);
    std::fs::create_dir_all(model.spec.unit.sandbox.read.first().expect("bind")).expect("dir");
    assert_eq!(
        model.weights(),
        Weights::Present,
        "a download that finishes shows at once"
    );
}

#[test]
fn the_card_offers_what_the_entry_can_do() {
    let scratch = Scratch::new("local-card");
    let models = models(&scratch);
    let kinds = |model: &LocalModel| -> Vec<&'static str> {
        model
            .card
            .capabilities
            .iter()
            .map(|c| match c {
                Capability::Llm(_) => "llm",
                Capability::ComputerUse(_) => "computer_use",
                Capability::Embeddings(_) => "embeddings",
                _ => "other",
            })
            .collect()
    };
    assert_eq!(kinds(&models[0]), ["llm"]);
    assert_eq!(kinds(&models[1]), ["embeddings"]);
    assert_eq!(kinds(&models[2]), ["llm", "computer_use"]);
    let Capability::Llm(llm) = &models[0].card.capabilities[0] else {
        panic!("llm");
    };
    assert!(
        llm.features.contains(&LlmFeature::PromptCache),
        "llama-server keeps its prompt cache"
    );
    assert_eq!(models[0].model_ref().model.as_str(), "tiny-chat");
}
