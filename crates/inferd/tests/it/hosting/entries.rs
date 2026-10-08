//! Catalog entries for the hosted tests: a chat model on llama-server, an embedding model, and a
//! computer-use model on vLLM. Not the shipped ones: the shipped files are read by `catalog`'s own
//! tests.

const SAMPLING: &str = r#"sampling = { reasoning_on = { temperature = 600, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } }, reasoning_off = { temperature = 0, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } }, reasoning_default = "off" }"#;

pub fn chat() -> String {
    format!(
        r#"id = "tiny-chat"
label = "Tiny chat"
licence = {{ kind = "open", v = "MIT" }}
family = "test"
cold_start_estimate_s = 0
source = {{ kind = "hugging_face", v = {{ repo = "test/tiny-chat", revision = "0000000000000000000000000000000000000001" }} }}
vram = {{ weights_mib = 100, kv_per_1k_ctx_mib = 1, overhead_mib = 50 }}
roles = ["llm"]
inputs = ["text", "image"]
tools = "server_parsed"
output = ["json_schema"]
reasoning = "absent"
streaming = "present"
context = 8192
max_output = 1024
{SAMPLING}
images = {{ per_prompt = 1, rule = {{ kind = "identity" }}, space = {{ kind = "image" }} }}
computer_use = {{ kind = "absent" }}

[[engine]]
kind = "llama_server"
args = []
weights = {{ kind = "gguf", v = {{ model = "m.gguf", mmproj = "p.gguf" }} }}
"#
    )
}

/// An embedding-only entry: the `embed` table is all it needs (no chat fields).
pub fn embed() -> String {
    r#"id = "tiny-embed"
label = "Tiny embedder"
licence = { kind = "open", v = "MIT" }
family = "test"
cold_start_estimate_s = 0
source = { kind = "hugging_face", v = { repo = "test/tiny-embed", revision = "0000000000000000000000000000000000000002" } }
vram = { weights_mib = 50, kv_per_1k_ctx_mib = 1, overhead_mib = 50 }
roles = ["embeddings"]
embed = { dims = 4, max_batch = 2, max_input = 512, prompts = { query = "search_query: ", document = "search_document: " } }

[[engine]]
kind = "llama_server"
args = ["--embeddings"]
weights = { kind = "gguf", v = { model = "e.gguf", mmproj = "" } }
"#
    .to_string()
}

pub fn cua() -> String {
    format!(
        r#"id = "tiny-cua"
label = "Tiny computer use"
licence = {{ kind = "open", v = "MIT" }}
family = "test"
cold_start_estimate_s = 0
source = {{ kind = "hugging_face", v = {{ repo = "test/tiny-cua", revision = "0000000000000000000000000000000000000003" }} }}
vram = {{ weights_mib = 100, kv_per_1k_ctx_mib = 1, overhead_mib = 50 }}
roles = ["llm", "computer_use"]
inputs = ["text", "image"]
tools = "server_parsed"
output = ["json_schema"]
reasoning = "absent"
streaming = "present"
context = 8192
max_output = 1024
{SAMPLING}
images = {{ per_prompt = 3, rule = {{ kind = "smart_resize", v = {{ factor = 32, min_pixels = 65536, max_pixels = 1048576 }} }}, space = {{ kind = "grid", v = 1000 }} }}
computer_use = {{ kind = "dialect", v = {{ dialect = {{ kind = "tool", v = "holo31" }}, batching = "one", zoom = "absent" }} }}

[[engine]]
kind = "vllm"
args = ["--max-model-len", "8192"]
weights = {{ kind = "hf_snapshot" }}
"#
    )
}

/// A hosted entry: `reaches` are (provider, the model's id there, wire, input and output price
/// per million tokens in micro-dollars). It takes text and images, calls tools natively and
/// writes no sampling, as the shipped hosted entries do.
pub fn hosted(
    id: &str,
    label: &str,
    family: &str,
    reaches: &[(&str, &str, &str, u64, u64)],
) -> String {
    let reach = reaches
        .iter()
        .map(|(provider, model, wire, input, output)| {
            format!(
                "  {{ provider = \"{provider}\", model = \"{model}\", price = {{ input_per_mtok = {input}, output_per_mtok = {output} }}, wire = \"{wire}\" }},\n"
            )
        })
        .collect::<String>();
    format!(
        r#"id = "{id}"
label = "{label}"
licence = {{ kind = "proprietary" }}
family = "{family}"
cold_start_estimate_s = 0
source = {{ kind = "hosted" }}
vram = {{ weights_mib = 0, kv_per_1k_ctx_mib = 0, overhead_mib = 0 }}
locality = {{ kind = "remote", v = {{ reach = [
{reach}] }} }}
inputs = ["text", "image"]
outputs = ["text"]
text_out = {{ tools = "native", structured = [], reasoning = "present", streaming = "present", context = 200000, max_output = 64000 }}
image_in = {{ per_prompt = 1, rule = {{ kind = "identity" }}, space = {{ kind = "image" }} }}
"#
    )
}

/// Claude Opus 5.5 as the shipped catalogue reaches it: Anthropic's own account, and OpenRouter.
pub fn claude() -> String {
    hosted(
        "claude-opus-5.5",
        "Claude Opus 5.5",
        "claude",
        &[
            (
                "anthropic",
                "claude-opus-5-5",
                "anthropic_messages",
                4_000_000,
                20_000_000,
            ),
            (
                "openrouter",
                "anthropic/claude-opus-5.5",
                "open_ai_compat",
                4_000_000,
                20_000_000,
            ),
        ],
    )
}

/// GPT-6 Luna: OpenAI's own account, and OpenRouter.
pub fn luna() -> String {
    hosted(
        "gpt-6-luna",
        "GPT-6 Luna",
        "gpt",
        &[
            ("openai", "gpt-6-luna", "open_ai_compat", 100_000, 500_000),
            (
                "openrouter",
                "openai/gpt-6-luna",
                "open_ai_compat",
                100_000,
                500_000,
            ),
        ],
    )
}

/// Kimi K3: Moonshot's own account, and OpenRouter.
pub fn kimi() -> String {
    hosted(
        "kimi-k3",
        "Kimi K3",
        "kimi",
        &[
            ("moonshot", "kimi-k3", "open_ai_compat", 600_000, 2_500_000),
            (
                "openrouter",
                "moonshotai/kimi-k3",
                "open_ai_compat",
                600_000,
                2_500_000,
            ),
        ],
    )
}

/// A speech-to-text entry on the speech host, as the shipped Nemotron entry is written (CPU only:
/// no VRAM, 16 kHz, a 560 ms chunk and six threads in its engine args).
pub fn speech_in() -> String {
    r#"id = "tiny-ears"
label = "Tiny ears"
licence = { kind = "open", v = "MIT" }
family = "test"
cold_start_estimate_s = 0
source = { kind = "hugging_face", v = { repo = "test/tiny-ears", revision = "0000000000000000000000000000000000000004" } }
vram = { weights_mib = 0, kv_per_1k_ctx_mib = 0, overhead_mib = 0 }
inputs = ["audio"]
outputs = ["text"]
text_out = { tools = "absent", structured = [], reasoning = "absent", streaming = "present", context = 0, max_output = 0 }
audio_in = { dir = "in", streaming = "present", partials = "present", punctuation = "present", timestamps = "present", langs = { kind = "listed", v = ["en-US", "zh-CN"] }, max_audio_ms = 120000, input = { rate = 16000, pcm = "s16_le" } }

[[engine]]
kind = "speech_host"
args = ["--socket", "{socket}", "--threads", "6", "--chunk-ms", "560"]
weights = { kind = "sherpa_dir" }
"#
    .to_string()
}

/// The id of the shipped attached entry, served by an engine on another machine.
pub const ATTACHED: &str = "qwen3.5-35b-a3b-fp8";

/// The name the lab serves it under in the tests (the shipped entry's own is its id): what a
/// request names and `/v1/models` lists.
pub const SERVED: &str = "lab-qwen";

/// The shipped entry of an attached engine (a vendored copy of stoker's
/// `catalog/qwen3.5-35b-a3b-fp8.toml` in `tests/fixtures`, so a plain clone builds: serving
/// attached, locality on-device, no engine profile), with the served name `SERVED`.
pub fn attached() -> String {
    include_str!("../../fixtures/qwen3.5-35b-a3b-fp8.toml").replace(
        "served_name = \"qwen3.5-35b-a3b-fp8\"",
        &format!("served_name = \"{SERVED}\""),
    )
}
