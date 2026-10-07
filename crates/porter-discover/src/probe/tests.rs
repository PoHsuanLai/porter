//! Port probes against recorded answers.

use super::*;
use crate::testing::{Table, respond};
use porter_core::capability::CapabilityKind;

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

fn model_ids(claims: &[Claim]) -> Vec<(String, CapabilityKind)> {
    claims
        .iter()
        .map(|c| match &c.subject {
            Subject::Model(id) => (id.to_string(), c.offer.kind()),
            Subject::Account | Subject::Agent(_) => panic!("{c:?}"),
        })
        .collect()
}

fn llm(claim: &Claim) -> &LlmCap {
    match &claim.offer {
        Offer::Present(Capability::Llm(llm)) => llm,
        other => panic!("{other:?}"),
    }
}

fn ollama() -> Table {
    Table::default()
        .get(
            &url(11434, "/api/tags"),
            respond(200, include_str!("../../fixtures/ollama-tags.json")),
        )
        .with(
            Method::Post,
            &url(11434, "/api/show"),
            respond(200, include_str!("../../fixtures/ollama-show-llama.json")),
        )
}

#[tokio::test]
async fn ollama_models_come_from_tags_and_show() {
    // One `/api/show` answer serves both names here (the table is keyed by URL); the llama
    // answer is a chat model with tools.
    let hits = probe_ports(&ollama(), &[Port(11434)]).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].port, Port(11434));
    assert_eq!(
        model_ids(&hits[0].claims),
        [
            ("llama3.2-3b".to_owned(), CapabilityKind::Llm),
            ("nomic-embed-text-latest".to_owned(), CapabilityKind::Llm),
        ]
    );
    let cap = llm(&hits[0].claims[0]);
    assert_eq!(cap.context, Tokens(131_072));
    assert_eq!(cap.wire, LlmWire::OllamaNative);
    assert_eq!(
        cap.features,
        BTreeSet::from([LlmFeature::Chat, LlmFeature::Tools])
    );
    assert!(
        hits[0]
            .claims
            .iter()
            .all(|c| c.provenance == Provenance::Discovered)
    );
}

#[test]
fn an_embedding_model_is_an_embedding_claim_with_its_dimensions() {
    let show: Value =
        serde_json::from_str(include_str!("../../fixtures/ollama-show-embed.json")).expect("json");
    let id = model_id("nomic-embed-text:latest").expect("id");
    let claims = ollama_model(id, Some(&show));
    assert_eq!(claims.len(), 1);
    let Offer::Present(Capability::Embeddings(embed)) = &claims[0].offer else {
        panic!("{:?}", claims[0])
    };
    assert_eq!(embed.dims, Dims(768));
    assert_eq!(embed.max_input, Tokens(2048));
    assert_eq!(embed.modalities, BTreeSet::from([Modality::Text]));
}

#[test]
fn a_model_whose_show_could_not_be_read_gets_the_chat_floor() {
    let claims = ollama_model(model_id("mystery").expect("id"), None);
    assert_eq!(claims.len(), 1);
    let cap = llm(&claims[0]);
    assert_eq!(cap.context, FLOOR_CONTEXT);
    assert_eq!(cap.features, BTreeSet::from([LlmFeature::Chat]));
}

#[test]
fn vision_and_thinking_are_features_and_an_embedding_without_dimensions_is_dropped() {
    let show = json!({"capabilities":["completion","vision","thinking","embedding"],
        "model_info":{"general.architecture":"x","x.context_length":4096}});
    let claims = ollama_model(model_id("m").expect("id"), Some(&show));
    assert_eq!(
        claims.len(),
        1,
        "no embedding_length, so no embedding claim"
    );
    assert_eq!(
        llm(&claims[0]).features,
        BTreeSet::from([LlmFeature::Chat, LlmFeature::Vision, LlmFeature::Reasoning])
    );
}

#[tokio::test]
async fn llama_cpp_answers_props_and_a_missing_tags_is_not_an_error() {
    let http = Table::default().get(
        &url(8080, "/props"),
        respond(200, include_str!("../../fixtures/llama-cpp-props.json")),
    );
    let hits = probe_ports(&http, &[Port(8080)]).await;
    assert_eq!(
        model_ids(&hits[0].claims),
        [(
            "qwen2.5-7b-instruct-q4_k_m.gguf".to_owned(),
            CapabilityKind::Llm
        )]
    );
    let cap = llm(&hits[0].claims[0]);
    assert_eq!(cap.context, Tokens(8192));
    assert_eq!(cap.wire, LlmWire::ChatCompletions);
    assert_eq!(http.seen()[0], format!("GET {}", url(8080, "/api/tags")));
}

#[tokio::test]
async fn an_openai_compatible_list_gives_chat_models_of_the_floor_context() {
    let http = Table::default().get(
        &url(1234, "/v1/models"),
        respond(200, include_str!("../../fixtures/openai-models.json")),
    );
    let hits = probe_ports(&http, &[Port(1234)]).await;
    assert_eq!(hits[0].claims.len(), 2);
    assert_eq!(llm(&hits[0].claims[0]).context, FLOOR_CONTEXT);
}

#[tokio::test]
async fn ports_are_probed_in_order_and_what_refuses_or_babbles_is_skipped() {
    let http = ollama()
        .get(&url(9999, "/api/tags"), respond(404, ""))
        .get(&url(9999, "/props"), respond(200, "<html>not json</html>"))
        .get(&url(9999, "/v1/models"), respond(500, ""));
    let hits = probe_ports(&http, &[Port(9998), Port(9999), Port(11434)]).await;
    assert_eq!(hits.len(), 1, "only the Ollama answered");
    assert_eq!(hits[0].port, Port(11434));
    assert!(probe_ports(&http, &[]).await.is_empty());
}

#[test]
fn model_names_become_ids_and_what_cannot_is_dropped() {
    assert_eq!(model_id("Llama3.2:3B").expect("id").as_str(), "llama3.2-3b");
    assert_eq!(
        model_id("hf.co/user/Model_Q4").expect("id").as_str(),
        "hf.co-user-model_q4"
    );
    assert!(model_id("").is_none());
    assert!(model_id("-leading").is_none());
    assert_eq!(model_id(&"a".repeat(100)).expect("id").as_str().len(), 64);
}
