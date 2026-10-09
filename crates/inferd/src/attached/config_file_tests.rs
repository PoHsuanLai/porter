//! The attached tables as `inferd.toml` holds them: the file's reading and the typed error it
//! refuses with. The tables' own checks are `porter_router::attached::config`'s tests.

use super::{AttachedError, Place};
use crate::config::{ConfigError, InferdConfig};
use std::path::Path;

#[test]
fn the_file_reads_attached_tables_beside_the_replay_engines_and_the_programs() {
    let text = r#"
[engines]
llama_server = "/usr/bin/llama-server"

[engines.scripted]
replay = "/tmp/cassette.jsonl"

[engines.attached."qwen3-32b"]
socket = "/run/user/1000/lab-vllm.sock"
key_file = "/home/me/.config/lab/vllm.key"
where = "my-network"

[engines.attached."small-local"]
url = "http://127.0.0.1:8001"
where = "this-device"
"#;
    let config = InferdConfig::from_toml(text).expect("reads");
    assert_eq!(
        config.engines.named.keys().collect::<Vec<_>>(),
        ["scripted"]
    );
    let attached = config.engines.attached().expect("checked");
    let ids: Vec<&str> = attached.iter().map(|one| one.id.as_str()).collect();
    assert_eq!(ids, ["qwen3-32b", "small-local"]);
    assert_eq!(attached[0].place, Place::MyNetwork);
    assert_eq!(
        attached[0].key_file.as_deref(),
        Some(Path::new("/home/me/.config/lab/vllm.key"))
    );
    // Written back (a Settings write rewrites the file) it reads the same.
    let again = toml::to_string(&config).expect("writes");
    assert_eq!(InferdConfig::from_toml(&again).expect("reads"), config);
}

#[test]
fn a_bad_attached_table_refuses_the_file_with_the_typed_error() {
    let missing = "[engines.attached.\"m\"]\nsocket = \"/a.sock\"\n";
    assert_eq!(
        InferdConfig::from_toml(missing),
        Err(ConfigError::Attached(AttachedError::MissingWhere {
            id: "m".into()
        }))
    );
    let remote =
        "[engines.attached.\"m\"]\nurl = \"http://10.0.0.5:8000\"\nwhere = \"my-network\"\n";
    assert_eq!(
        InferdConfig::from_toml(remote),
        Err(ConfigError::Attached(AttachedError::NotLoopback {
            id: "m".into(),
            host: "10.0.0.5".into()
        }))
    );
}
