//! The model a replay engine stands in for: a catalog entry written here (a text-and-tools chat
//! model with structured output, which is what the cassette's replies are), so routing sees a
//! card like any other, and the engine's spec and socket like any other. Its figures are zero:
//! it takes no GPU memory and no weights.

use crate::catalog::parse_entry_text;
use crate::local::LocalModel;
use engine_supervisor::{EngineId, EnginePaths, EngineSpec, ProgramPath, SocketPath, command};
use model_catalog::EngineKind;
use model_openai_compat::Flavor;
use model_provider::{ModelName, Tokens};
use porter_core::{AccountId, Billing, Capability, Locality, ModelId, Offer};
use porter_infer::ModelCard;
use std::path::{Path, PathBuf};

const SAMPLING: &str = r#"sampling = { reasoning_on = { temperature = 600, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } }, reasoning_off = { temperature = 0, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } }, reasoning_default = "off" }"#;

/// The catalog text of a replay model named `name` with this context.
fn entry_text(name: &str, context: Tokens) -> String {
    format!(
        r#"id = "{name}"
label = "Replay {name}"
licence = {{ kind = "open", v = "MIT" }}
family = "replay"
cold_start_estimate_s = 0
source = {{ kind = "hugging_face", v = {{ repo = "replay/{name}", revision = "0000000000000000000000000000000000000000" }} }}
vram = {{ weights_mib = 0, kv_per_1k_ctx_mib = 0, overhead_mib = 0 }}
roles = ["llm"]
inputs = ["text"]
tools = "server_parsed"
output = ["json_schema"]
reasoning = "absent"
streaming = "present"
context = {}
max_output = 4096
{SAMPLING}
images = {{ per_prompt = 1, rule = {{ kind = "identity" }}, space = {{ kind = "image" }} }}
computer_use = {{ kind = "absent" }}

[[engine]]
kind = "llama_server"
args = []
weights = {{ kind = "gguf", v = {{ model = "replay.gguf", mmproj = "" }} }}
"#,
        context.0
    )
}

/// The local model of replay engine `name`, listening on `sockets/replay-<name>.sock`, playing
/// the cassette at `cassette`. `None` when `name` is not a model id.
pub fn model(name: &str, cassette: &Path, context: Tokens, sockets: &Path) -> Option<LocalModel> {
    let id = ModelId::parse(name).ok()?;
    let entry = parse_entry_text(&entry_text(name, context)).ok()?;
    let capabilities: Vec<Capability> = crate::catalog::claims_of(&entry)
        .into_iter()
        .filter_map(|claim| match claim.offer {
            Offer::Present(capability) => Some(capability),
            Offer::Absent { .. } => None,
        })
        .collect();
    let profile = entry.engines.first()?.clone();
    let socket = SocketPath(sockets.join(format!("replay-{name}.sock")));
    let paths = EnginePaths {
        vllm_python: ProgramPath(PathBuf::new()),
        llama_server: ProgramPath(PathBuf::from("replay")),
        speech_host: ProgramPath(PathBuf::new()),
        kokoro_python: ProgramPath(PathBuf::new()),
        hf_cache: PathBuf::new(),
    };
    debug_assert_eq!(profile.kind, EngineKind::LlamaServer);
    Some(LocalModel {
        card: ModelCard {
            account: AccountId::parse(crate::local::LOCAL_ACCOUNT).ok()?,
            model: id,
            locality: Locality::OnDevice,
            billing: Billing::Free,
            capabilities,
        },
        name: ModelName(name.to_owned()),
        spec: EngineSpec {
            id: EngineId(format!("replay:{name}")),
            need: model_catalog::MiB(0),
            unit: command(&entry, &profile, &paths, &socket),
        },
        socket,
        flavor: Some(Flavor::LlamaServer),
        cassette: Some(cassette.to_path_buf()),
        entry,
        profile,
    })
}

#[cfg(test)]
mod tests;
