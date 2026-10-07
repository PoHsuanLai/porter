use super::*;
use crate::config::{ConfigError, InferdConfig};

fn entry(text: &str) -> AttachedEntry {
    toml::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

#[test]
fn a_url_is_plain_http_on_loopback_and_nothing_else() {
    let loopback = |host: [u8; 4], port: u16| Reach::Loopback {
        host: Ipv4Addr::from(host),
        port: Port(port),
    };
    let table = [
        ("http://127.0.0.1:8000", Ok(loopback([127, 0, 0, 1], 8000))),
        ("http://127.0.0.1:8000/", Ok(loopback([127, 0, 0, 1], 8000))),
        (
            "http://localhost:11434",
            Ok(loopback([127, 0, 0, 1], 11434)),
        ),
        ("HTTP://LocalHost:9", Ok(loopback([127, 0, 0, 1], 9))),
        ("http://127.0.0.2:80", Ok(loopback([127, 0, 0, 2], 80))),
        (
            "http://192.168.1.20:8000",
            Err(AttachedError::NotLoopback {
                id: "m".into(),
                host: "192.168.1.20".into(),
            }),
        ),
        (
            "http://lab.example.org:8000",
            Err(AttachedError::NotLoopback {
                id: "m".into(),
                host: "lab.example.org".into(),
            }),
        ),
        (
            "http://0.0.0.0:8000",
            Err(AttachedError::NotLoopback {
                id: "m".into(),
                host: "0.0.0.0".into(),
            }),
        ),
        (
            "http://[::1]:8000",
            Err(AttachedError::NotLoopback {
                id: "m".into(),
                host: "[::1]".into(),
            }),
        ),
        (
            "http://127.0.0.1.evil.example:8000",
            Err(AttachedError::NotLoopback {
                id: "m".into(),
                host: "127.0.0.1.evil.example".into(),
            }),
        ),
        (
            "https://127.0.0.1:8000",
            Err(AttachedError::NotPlainHttp { id: "m".into() }),
        ),
        (
            "http://127.0.0.1",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
        (
            "http://127.0.0.1:0",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
        (
            "http://127.0.0.1:99999",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
        (
            "http://127.0.0.1:8000/v1",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
        (
            "http://user@127.0.0.1:8000",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
        (
            "127.0.0.1:8000",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
        (
            "ftp://127.0.0.1:8000",
            Err(AttachedError::BadUrl { id: "m".into() }),
        ),
    ];
    for (url, want) in table {
        assert_eq!(url_reach("m", url), want, "{url}");
    }
}

#[test]
fn an_entry_names_one_target_and_where_the_data_goes() {
    let table = [
        (
            "socket = \"/run/user/1000/lab.sock\"\nwhere = \"my-network\"",
            Ok((
                Reach::Socket("/run/user/1000/lab.sock".into()),
                Place::MyNetwork,
            )),
        ),
        (
            "url = \"http://127.0.0.1:8000\"\nwhere = \"this-device\"\nkey_file = \"/k/key\"",
            Ok((
                Reach::Loopback {
                    host: Ipv4Addr::LOCALHOST,
                    port: Port(8000),
                },
                Place::ThisDevice,
            )),
        ),
        (
            "socket = \"/run/lab.sock\"",
            Err(AttachedError::MissingWhere { id: "m".into() }),
        ),
        (
            "where = \"this-device\"",
            Err(AttachedError::NoTarget { id: "m".into() }),
        ),
        (
            "socket = \"/a.sock\"\nurl = \"http://127.0.0.1:1\"\nwhere = \"this-device\"",
            Err(AttachedError::BothTargets { id: "m".into() }),
        ),
        (
            "socket = \"lab.sock\"\nwhere = \"this-device\"",
            Err(AttachedError::RelativeSocket {
                id: "m".into(),
                path: "lab.sock".into(),
            }),
        ),
        (
            "socket = \"/a.sock\"\nwhere = \"this-device\"\nkey_file = \"key\"",
            Err(AttachedError::RelativeKeyFile {
                id: "m".into(),
                path: "key".into(),
            }),
        ),
    ];
    for (text, want) in table {
        let got = entry(text)
            .check("m")
            .map(|attached| (attached.reach, attached.place));
        assert_eq!(got, want, "{text}");
    }
}

#[test]
fn a_socket_path_too_long_to_connect_to_is_refused() {
    let long = format!("/{}", "a".repeat(120));
    let text = format!("socket = \"{long}\"\nwhere = \"this-device\"");
    assert!(matches!(
        entry(&text).check("m"),
        Err(AttachedError::SocketPathTooLong { len: 121, .. })
    ));
}

#[test]
fn a_table_whose_name_is_not_a_model_id_is_refused() {
    let one = entry("socket = \"/a.sock\"\nwhere = \"this-device\"");
    assert_eq!(
        one.check("Not A Model!"),
        Err(AttachedError::BadId {
            id: "Not A Model!".into()
        })
    );
}

#[test]
fn where_has_no_default_and_only_two_values_and_an_unknown_key_does_not_parse() {
    assert!(toml::from_str::<AttachedEntry>("where = \"lab\"").is_err());
    assert!(toml::from_str::<AttachedEntry>("where = \"this-device\"\nhost = \"x\"").is_err());
    assert_eq!(entry("").place, None);
}

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
