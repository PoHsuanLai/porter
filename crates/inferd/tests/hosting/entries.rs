//! Catalog entries for the hosted tests: a chat model on llama-server, an embedding model, and a
//! computer-use model on vLLM. Not the shipped ones: the shipped files are read by `catalog`'s own
//! tests.

const SAMPLING: &str = r#"sampling = { reasoning_on = { temperature = 600, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } }, reasoning_off = { temperature = 0, top_p = { kind = "off" }, top_k = { kind = "off" }, min_p = { kind = "off" }, repeat_penalty = { kind = "off" }, seed = { kind = "off" } } }"#;

pub fn chat() -> String {
    format!(
        r#"id = "tiny-chat"
label = "Tiny chat"
licence = {{ kind = "open", v = "MIT" }}
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

pub fn embed() -> String {
    format!(
        r#"id = "tiny-embed"
label = "Tiny embedder"
licence = {{ kind = "open", v = "MIT" }}
source = {{ kind = "hugging_face", v = {{ repo = "test/tiny-embed", revision = "0000000000000000000000000000000000000002" }} }}
vram = {{ weights_mib = 50, kv_per_1k_ctx_mib = 1, overhead_mib = 50 }}
roles = ["embeddings"]
inputs = ["text"]
tools = "absent"
output = []
reasoning = "absent"
streaming = "absent"
context = 2048
max_output = 1
{SAMPLING}
images = {{ per_prompt = 0, rule = {{ kind = "identity" }}, space = {{ kind = "image" }} }}
computer_use = {{ kind = "absent" }}

[[engine]]
kind = "llama_server"
args = []
weights = {{ kind = "gguf", v = {{ model = "e.gguf", mmproj = "p.gguf" }} }}
"#
    )
}

pub fn cua() -> String {
    format!(
        r#"id = "tiny-cua"
label = "Tiny computer use"
licence = {{ kind = "open", v = "MIT" }}
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
