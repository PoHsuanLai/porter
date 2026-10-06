use super::names::{Listing, id_of, pair};
use super::testing::{chat, embedding, found};
use super::*;
use porter_core::capability::{Capability, LlmFeature, LlmWire};
use porter_core::{AccountId, Locality};
use porter_fake_servers::{FakeModels, ModelDef, Wire};
use porter_http::HyperHttp;
use serde_json::json;

fn http() -> HyperHttp {
    HyperHttp::new().with_limits(porter_http::Limits {
        timeout: Duration::from_secs(5),
        max_body: 1 << 20,
    })
}

/// A loopback port nothing listens on: one this test bound and let go.
fn closed_port() -> u16 {
    let socket = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    socket.local_addr().expect("addr").port()
}

fn only(runtime: Runtime, ports: &[u16]) -> ProbeConfig {
    let mut config = ProbeConfig::off();
    let ports = ports.to_vec();
    match runtime {
        Runtime::Ollama => config.ollama = ports,
        Runtime::LlamaCpp => config.llama_cpp = ports,
        Runtime::LmStudio => config.lm_studio = ports,
    }
    config
}

#[test]
fn the_defaults_are_the_three_runtimes_ports_and_the_table_is_left_out_of_the_file_while_it_is_the_default()
 {
    let config = ProbeConfig::default();
    assert_eq!(
        Runtime::ALL.map(|runtime| config.ports(runtime).to_vec()),
        [vec![11_434], vec![8080], vec![1234]]
    );
    assert!(config.is_default());
    assert!(!ProbeConfig::off().is_default());
    let file = crate::config::InferdConfig::default();
    let text = toml::to_string(&file).expect("serialises");
    assert!(!text.contains("probe"), "{text}");
}

#[test]
fn the_probe_table_names_ports_and_a_runtime_with_none_is_not_probed() {
    let config: ProbeConfig =
        toml::from_str("ollama = [31000]\nllama_cpp = []\nevery_s = 3\n").expect("parses");
    assert_eq!(config.ports(Runtime::Ollama), [31_000]);
    assert_eq!(config.ports(Runtime::LlamaCpp), [] as [u16; 0]);
    // What the table does not say is the default.
    assert_eq!(config.ports(Runtime::LmStudio), [1234]);
    assert_eq!(config.base(), Duration::from_secs(3));
    assert_eq!(config.longest(), Duration::from_secs(120));
    assert!(toml::from_str::<ProbeConfig>("ollama = [1]\nhost = \"10.0.0.2\"\n").is_err());
}

#[test]
fn the_longest_wait_is_never_shorter_than_the_base() {
    let config: ProbeConfig = toml::from_str("every_s = 30\nlongest_s = 5\n").expect("parses");
    assert_eq!(config.longest(), Duration::from_secs(30));
}

#[test]
fn a_list_answer_names_its_models_by_the_runtimes_own_keys() {
    let ollama = json!({ "models": [
        { "name": "llama3.2:3b", "model": "llama3.2:3b" }, { "model": "nomic-embed-text:latest" }, { "size": 1 }
    ] });
    let openai =
        json!({ "object": "list", "data": [{ "id": "qwen2.5-7b" }, { "id": "/models/x.gguf" }] });
    let table = [
        (
            Runtime::Ollama,
            &ollama,
            vec!["llama3.2:3b", "nomic-embed-text:latest"],
        ),
        (
            Runtime::LmStudio,
            &openai,
            vec!["qwen2.5-7b", "/models/x.gguf"],
        ),
        (
            Runtime::LlamaCpp,
            &openai,
            vec!["qwen2.5-7b", "/models/x.gguf"],
        ),
        // The wrong shape is no names.
        (Runtime::Ollama, &openai, vec![]),
    ];
    for (runtime, body, want) in table {
        assert_eq!(Listing::of(runtime, body).0, want, "{runtime:?}");
    }
}

#[test]
fn a_name_makes_the_id_the_probe_gave_its_claim() {
    let table = [
        ("llama3.2:3b", Some("llama3.2-3b")),
        ("Llama3.2:3B", Some("llama3.2-3b")),
        (
            "hf.co/org/Model-GGUF:Q4_K_M",
            Some("hf.co-org-model-gguf-q4_k_m"),
        ),
        ("", None),
    ];
    for (name, want) in table {
        assert_eq!(id_of(name).as_ref().map(ModelId::as_str), want, "{name}");
    }
}

#[test]
fn each_claim_is_paired_with_the_name_that_makes_its_id() {
    let first = chat("llama3.2-3b", "x", 8192, &[LlmFeature::Chat]);
    let second = chat("qwen2.5-7b", "x", 4096, &[LlmFeature::Chat]);
    let claims: Vec<_> = first.claims.iter().chain(&second.claims).cloned().collect();
    let listing = Listing(vec!["Qwen2.5-7B".into(), "llama3.2:3b".into()]);
    let paired = pair(&listing, &claims);
    let names: Vec<_> = paired
        .iter()
        .map(|model| (model.id.as_str(), model.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [("llama3.2-3b", "llama3.2:3b"), ("qwen2.5-7b", "Qwen2.5-7B")]
    );
    assert!(paired.iter().all(|model| model.claims.len() == 1));
}

#[test]
fn one_model_listed_under_a_path_is_the_one_model_found_under_its_alias() {
    // llama.cpp's /props names the file's basename; /v1/models names the path.
    let model = chat("model.gguf", "x", 4096, &[LlmFeature::Chat]);
    let paired = pair(
        &Listing(vec!["/models/other-name.gguf".into()]),
        &model.claims,
    );
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].name, "/models/other-name.gguf");
}

#[test]
fn a_claim_no_name_makes_is_left_out_when_there_are_many_names() {
    let a = chat("a-model", "x", 4096, &[LlmFeature::Chat]);
    let b = chat("b-model", "x", 4096, &[LlmFeature::Chat]);
    let claims: Vec<_> = a.claims.iter().chain(&b.claims).cloned().collect();
    let paired = pair(&Listing(vec!["a-model".into(), "c-model".into()]), &claims);
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].id.as_str(), "a-model");
}

#[test]
fn a_probed_chat_model_is_an_on_device_free_card_on_the_runtimes_account() {
    let model = chat(
        "llama3.2-3b",
        "llama3.2:3b",
        8192,
        &[LlmFeature::Chat, LlmFeature::Tools],
    );
    let local = found(Runtime::Ollama, 11_434, vec![model])
        .local_models(std::path::Path::new("/nonexistent"))
        .remove(0);
    assert_eq!(local.card.account, AccountId::parse("ollama").expect("id"));
    assert_eq!(local.card.locality, Locality::OnDevice);
    assert_eq!(local.card.billing, porter_core::Billing::Free);
    assert_eq!(local.name.0, "llama3.2:3b");
    assert_eq!(local.loopback, Some(model_http::Port(11_434)));
    assert_eq!(local.weights(), crate::local::Weights::Present);
    // Every runtime is spoken to in chat completions, whatever the probe's source said.
    let Some(Capability::Llm(cap)) = local.card.capabilities.first() else {
        panic!("an Llm card");
    };
    assert_eq!(cap.wire, LlmWire::ChatCompletions);
    assert!(cap.features.contains(&LlmFeature::Tools));
    // The entry the turn code reads carries the context and tool support the probe found.
    let caps = local.caps().expect("chat caps");
    assert_eq!((caps.context.0, caps.max_output.0), (8192, 8192));
    assert_eq!(caps.tools, model_provider::ToolSupport::ServerParsed);
    assert_eq!(local.entry.label, "llama3.2:3b");
}

#[test]
fn vision_and_reasoning_reach_the_entry_and_a_plain_model_has_no_tools() {
    let rich = chat(
        "vl",
        "vl",
        4096,
        &[LlmFeature::Chat, LlmFeature::Vision, LlmFeature::Reasoning],
    );
    let plain = chat("plain", "plain", 2048, &[LlmFeature::Chat]);
    let sockets = std::path::Path::new("/nonexistent");
    let made = found(Runtime::LmStudio, 1234, vec![rich, plain]).local_models(sockets);
    let caps = |index: usize| made[index].caps().expect("caps").clone();
    assert!(caps(0).inputs.contains(&model_provider::InputKind::Image));
    assert_eq!(caps(0).reasoning, model_provider::Support::Present);
    assert_eq!(caps(1).tools, model_provider::ToolSupport::Absent);
    assert!(!caps(1).inputs.contains(&model_provider::InputKind::Image));
    assert_eq!(made[1].card.account.as_str(), "lm-studio");
}

#[test]
fn a_probed_embedding_model_is_an_embeddings_card_with_its_width_and_prefix() {
    let model = embedding("nomic-embed-text", "nomic-embed-text:latest");
    let local = found(Runtime::Ollama, 11_434, vec![model])
        .local_models(std::path::Path::new("/nonexistent"))
        .remove(0);
    let Some(Capability::Embeddings(cap)) = local.card.capabilities.first() else {
        panic!("an Embeddings card");
    };
    assert_eq!(cap.dims.0, 768);
    let embed = local.embed().expect("embed caps");
    assert_eq!(embed.prompts.query.0, "search_query: ");
}

#[tokio::test]
async fn an_ollama_that_answers_is_found_with_its_models_under_the_names_a_request_needs() {
    let fake = FakeModels::start(
        Wire::Ollama,
        vec![
            ModelDef::chat("llama3.2:3b", 8192),
            ModelDef::embedding("nomic-embed-text:latest", 2048),
        ],
        None,
    )
    .await
    .expect("fake");
    let port: u16 = fake
        .base_url()
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .expect("number");
    let seen = probe(&http(), &only(Runtime::Ollama, &[port])).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (seen[0].runtime, seen[0].port),
        (Runtime::Ollama, Port(port))
    );
    let names: Vec<_> = seen[0]
        .models
        .iter()
        .map(|model| (model.id.as_str(), model.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("llama3.2-3b", "llama3.2:3b"),
            ("nomic-embed-text-latest", "nomic-embed-text:latest")
        ]
    );
    assert_eq!(seen[0].claims().len(), 2);
}

#[tokio::test]
async fn an_openai_compatible_server_is_found_on_the_port_its_runtime_is_configured_at() {
    let fake = FakeModels::start(
        Wire::OpenAi,
        vec![ModelDef::chat("qwen2.5-7b-instruct", 4096)],
        None,
    )
    .await
    .expect("fake");
    let port: u16 = fake
        .base_url()
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .expect("number");
    let seen = probe(&http(), &only(Runtime::LmStudio, &[port])).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].runtime, Runtime::LmStudio);
    assert_eq!(seen[0].models[0].name, "qwen2.5-7b-instruct");
    // The same server under another runtime's port list is that runtime's, and nothing else.
    let other = probe(&http(), &only(Runtime::LlamaCpp, &[port])).await;
    assert_eq!(other[0].runtime, Runtime::LlamaCpp);
}

#[tokio::test]
async fn a_port_nothing_listens_on_finds_nothing_and_a_runtime_with_no_ports_is_not_asked() {
    let closed = closed_port();
    assert_eq!(
        probe(&http(), &only(Runtime::Ollama, &[closed])).await,
        vec![]
    );
    let fake = FakeModels::start(Wire::Ollama, vec![ModelDef::chat("m", 2048)], None)
        .await
        .expect("fake");
    assert_eq!(probe(&http(), &ProbeConfig::off()).await, vec![]);
    assert!(
        fake.hits().is_empty(),
        "nothing was asked of a port that was not configured"
    );
}

#[tokio::test]
async fn the_first_of_a_runtimes_ports_that_answers_is_its_one_account() {
    let fake = FakeModels::start(Wire::Ollama, vec![ModelDef::chat("m", 2048)], None)
        .await
        .expect("fake");
    let port: u16 = fake
        .base_url()
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .expect("number");
    let seen = probe(&http(), &only(Runtime::Ollama, &[closed_port(), port])).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].port, Port(port));
}

#[tokio::test]
async fn a_port_another_runtime_already_answered_on_is_not_asked_again() {
    let fake = FakeModels::start(Wire::Ollama, vec![ModelDef::chat("m", 2048)], None)
        .await
        .expect("fake");
    let port: u16 = fake
        .base_url()
        .rsplit(':')
        .next()
        .expect("port")
        .parse()
        .expect("number");
    let mut config = ProbeConfig::off();
    config.ollama = vec![port];
    config.llama_cpp = vec![port];
    let seen = probe(&http(), &config).await;
    assert_eq!(
        seen.iter().map(|one| one.runtime).collect::<Vec<_>>(),
        [Runtime::Ollama]
    );
}
