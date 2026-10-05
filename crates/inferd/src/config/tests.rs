use super::*;
use porter_core::DataClass;
use porter_infer::{Floor, LocalOnly};
use std::collections::BTreeMap;

fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| map.get(name).cloned()
}

#[test]
fn the_directories_follow_xdg_and_fall_back_under_home() {
    let from_home = Dirs::from_vars(vars(&[
        ("HOME", "/home/u"),
        ("XDG_RUNTIME_DIR", "/run/user/1000"),
    ]))
    .expect("dirs");
    assert_eq!(
        from_home.config,
        PathBuf::from("/home/u/.config/quire/inferd.toml")
    );
    assert_eq!(
        from_home.catalog.system,
        PathBuf::from("/usr/share/stoker/catalog")
    );
    assert_eq!(
        from_home.catalog.user,
        PathBuf::from("/home/u/.local/share/stoker/catalog")
    );
    assert_eq!(from_home.sockets, PathBuf::from("/run/user/1000/inferd"));
    assert_eq!(
        from_home.audit,
        PathBuf::from("/home/u/.local/state/quire/inferd/audit.jsonl")
    );
    assert_eq!(
        from_home.hf_cache,
        PathBuf::from("/home/u/.cache/huggingface/hub")
    );

    let explicit = Dirs::from_vars(vars(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", "/cfg"),
        ("XDG_DATA_HOME", "/data"),
        ("XDG_STATE_HOME", "/state"),
        ("XDG_RUNTIME_DIR", "/run"),
        ("HF_HOME", "/hf"),
    ]))
    .expect("dirs");
    assert_eq!(explicit.config, PathBuf::from("/cfg/quire/inferd.toml"));
    assert_eq!(explicit.catalog.user, PathBuf::from("/data/stoker/catalog"));
    assert_eq!(
        explicit.audit,
        PathBuf::from("/state/quire/inferd/audit.jsonl")
    );
    assert_eq!(explicit.hf_cache, PathBuf::from("/hf/hub"));
}

#[test]
fn without_a_runtime_directory_or_a_home_there_is_nowhere_to_put_the_sockets() {
    assert_eq!(
        Dirs::from_vars(vars(&[("HOME", "/home/u")])),
        Err(ConfigError::NoDirs)
    );
    assert_eq!(
        Dirs::from_vars(vars(&[("XDG_RUNTIME_DIR", "/run")])),
        Err(ConfigError::NoDirs)
    );
}

#[test]
fn an_empty_file_is_the_default_and_the_default_has_the_proposed_policy() {
    let config = InferdConfig::from_toml("").expect("empty");
    assert_eq!(config, InferdConfig::default());
    assert_eq!(config.policy(), Policy::proposed());
    assert_eq!(config.policy().floor(DataClass::Prompt), Floor::OnDevice);
}

#[test]
fn every_table_of_the_file_reads() {
    let config = InferdConfig::from_toml(
        r#"
[engines]
vllm_python = "/opt/vllm/bin/python"
llama_server = "/usr/bin/llama-server"
hf_cache = "/data/hub"

[policy]
local_only = "off"
floors = [{ class = "mail", floor = "local_network" }]

[callers]
cua = ["/usr/libexec/quire/cuad"]
[callers.apps]
"org.quire.Memory" = ["/usr/libexec/quire/memoryd"]
"#,
    )
    .expect("config");
    assert_eq!(
        config.engines.vllm_python,
        Some(PathBuf::from("/opt/vllm/bin/python"))
    );
    assert_eq!(config.engines.speech_host, None);
    let policy = config.policy();
    assert_eq!(policy.local_only, LocalOnly::Off);
    assert_eq!(policy.floor(DataClass::Mail), Floor::LocalNetwork);
    assert_eq!(
        policy.floor(DataClass::Notes),
        Floor::Anywhere,
        "a class with no row may go anywhere"
    );
    assert!(
        config
            .callers
            .resolve(std::path::Path::new("/usr/libexec/quire/cuad"))
            .is_some()
    );
}

#[test]
fn a_file_that_does_not_read_says_why() {
    for text in [
        "[engines",
        "engines = 3",
        "[policy]\nlocal_only = \"maybe\"",
        "callers = 3",
    ] {
        assert!(
            matches!(InferdConfig::from_toml(text), Err(ConfigError::Toml(_))),
            "{text:?}"
        );
    }
}

#[test]
fn the_weights_cache_is_the_files_or_the_default() {
    let dirs =
        Dirs::from_vars(vars(&[("HOME", "/home/u"), ("XDG_RUNTIME_DIR", "/run")])).expect("dirs");
    let none = InferdConfig::default();
    assert_eq!(none.engines_in(&dirs).hf_cache, dirs.hf_cache);
    let own = InferdConfig::from_toml("[engines]\nhf_cache = \"/mine\"").expect("config");
    assert_eq!(own.engines_in(&dirs).hf_cache, PathBuf::from("/mine"));
}

#[test]
fn a_named_engine_with_a_cassette_reads_beside_the_programs() {
    let config = InferdConfig::from_toml(
        "[engines]\nllama_server = \"/usr/bin/llama-server\"\n[engines.scripted]\nreplay = \"/c/flow.jsonl\"\n",
    )
    .expect("config");
    assert_eq!(
        config.engines.llama_server,
        Some(PathBuf::from("/usr/bin/llama-server"))
    );
    assert_eq!(
        config.engines.named.get("scripted").map(|e| e.replay.clone()),
        Some(PathBuf::from("/c/flow.jsonl"))
    );
    assert!(InferdConfig::from_toml("[engines.x]\nnope = 1\n").is_err());
    assert!(InferdConfig::default().engines.named.is_empty());
}
