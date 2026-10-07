//! The local model one probed model is: a catalog entry written from what the probe read (as the
//! replay engine's is), so routing sees a card like any other and the turn code finds the
//! sampling, limits and tools it reads from an entry. Nothing starts: the model is served by the
//! runtime on its loopback port, its engine id names no process, and its card says `OnDevice`.

use super::{Probed, Runtime};
use crate::catalog::parse_entry_text;
use crate::local::LocalModel;
use engine_supervisor::{EngineId, EnginePaths, EngineSpec, ProgramPath, SocketPath, command};
use model_catalog::MiB;
use model_http::Port;
use model_openai_compat::Flavor;
use model_provider::ModelName;
use porter_core::capability::{Capability, EmbedCap, LlmCap, LlmFeature, LlmWire};
use porter_core::{AccountId, Billing, Locality, Offer};
use porter_infer::ModelCard;
use std::path::{Path, PathBuf};

const SAMPLING: &str = r#"sampling = { reasoning_on = { temperature = 600, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } }, reasoning_default = "off", reasoning_off = { temperature = 0, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } } }"#;

/// A TOML string, quoted.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

/// What the probe read of a model's chat side, as the catalog's fields.
fn chat_fields(cap: &LlmCap) -> String {
    let has = |feature| cap.features.contains(&feature);
    let inputs = match has(LlmFeature::Vision) {
        true => r#"["text", "image"]"#,
        false => r#"["text"]"#,
    };
    let tools = match has(LlmFeature::Tools) {
        true => "server_parsed",
        false => "absent",
    };
    let output = match has(LlmFeature::StructuredOutput) {
        true => r#"["json_schema"]"#,
        false => "[]",
    };
    let reasoning = match has(LlmFeature::Reasoning) {
        true => "present",
        false => "absent",
    };
    format!(
        r#"inputs = {inputs}
tools = "{tools}"
output = {output}
reasoning = "{reasoning}"
streaming = "present"
context = {}
max_output = {}
{SAMPLING}
images = {{ per_prompt = 1, rule = {{ kind = "identity" }}, space = {{ kind = "image" }} }}
computer_use = {{ kind = "absent" }}
"#,
        cap.context.0, cap.max_output.0
    )
}

fn embed_fields(cap: &EmbedCap) -> String {
    format!(
        "embed = {{ dims = {}, max_batch = {}, max_input = {}, prompts = {{ query = {}, document = {} }} }}\n",
        cap.dims.0,
        cap.max_batch.0.max(1),
        cap.max_input.0,
        quoted(&cap.prompts.query.0),
        quoted(&cap.prompts.document.0),
    )
}

/// The catalog text of a probed model.
fn entry_text(model: &Probed, llm: Option<&LlmCap>, embed: Option<&EmbedCap>) -> String {
    let roles: Vec<&str> = [llm.map(|_| "\"llm\""), embed.map(|_| "\"embeddings\"")]
        .into_iter()
        .flatten()
        .collect();
    let mut text = format!(
        r#"id = {}
label = {}
licence = {{ kind = "open", v = "unknown" }}
family = "probed"
cold_start_estimate_s = 0
source = {{ kind = "hugging_face", v = {{ repo = "probed/{}", revision = "0000000000000000000000000000000000000000" }} }}
vram = {{ weights_mib = 0, kv_per_1k_ctx_mib = 0, overhead_mib = 0 }}
roles = [{}]
"#,
        quoted(model.id.as_str()),
        quoted(&model.name),
        model.id.as_str(),
        roles.join(", "),
    );
    text.push_str(&llm.map(chat_fields).unwrap_or_default());
    text.push_str(&embed.map(embed_fields).unwrap_or_default());
    text.push_str(
        "\n[[engine]]\nkind = \"llama_server\"\nargs = []\nweights = { kind = \"gguf\", v = { model = \"probed.gguf\", mmproj = \"\" } }\n",
    );
    text
}

/// What a runtime's claims say a model can do, as the card lists it: the chat wire is the one
/// spoken to every runtime here (OpenAI-compatible chat completions), whatever the probe's
/// source said.
fn capabilities_of(model: &Probed) -> Vec<Capability> {
    model
        .claims
        .iter()
        .filter_map(|claim| match &claim.offer {
            Offer::Present(Capability::Llm(cap)) => Some(Capability::Llm(LlmCap {
                wire: LlmWire::ChatCompletions,
                ..cap.clone()
            })),
            Offer::Present(capability) => Some(capability.clone()),
            Offer::Absent { .. } => None,
        })
        .collect()
}

/// The local model of one probed model of `runtime`, on `port`. `sockets` is where an engine's
/// socket would go (the field is required of every model and unused here). `None` when the model
/// is neither a language nor an embedding model, or its entry does not parse.
pub fn local_model(
    runtime: Runtime,
    port: porter_provider::Port,
    model: &Probed,
    sockets: &Path,
) -> Option<LocalModel> {
    let capabilities = capabilities_of(model);
    let llm = capabilities.iter().find_map(|c| match c {
        Capability::Llm(cap) => Some(cap),
        _ => None,
    });
    let embed = capabilities.iter().find_map(|c| match c {
        Capability::Embeddings(cap) => Some(cap),
        _ => None,
    });
    if llm.is_none() && embed.is_none() {
        return None;
    }
    let entry = parse_entry_text(&entry_text(model, llm, embed)).ok()?;
    let profile = entry.engines.first()?.clone();
    let provider = runtime.provider();
    let socket = SocketPath(sockets.join(format!("probed-{provider}-{}.sock", model.id.as_str())));
    let paths = EnginePaths {
        vllm_python: ProgramPath(PathBuf::new()),
        llama_server: ProgramPath(PathBuf::from("probed")),
        speech_host: ProgramPath(PathBuf::new()),
        kokoro_python: ProgramPath(PathBuf::new()),
        hf_cache: PathBuf::new(),
    };
    Some(LocalModel {
        card: ModelCard {
            account: AccountId::parse(provider).ok()?,
            model: model.id.clone(),
            locality: Locality::OnDevice,
            billing: Billing::Free,
            capabilities,
        },
        name: ModelName(model.name.clone()),
        spec: EngineSpec {
            id: EngineId(format!("probed:{provider}:{}", model.id.as_str())),
            need: MiB(0),
            unit: command(&entry, &profile, &paths, &socket),
        },
        socket,
        flavor: Some(Flavor::LlamaServer),
        cassette: None,
        loopback: Some(Port(port.0)),
        attached: None,
        entry,
        profile,
    })
}
