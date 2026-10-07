use super::engine::Scheme;
use super::routes::Table;
use super::wire::{Temperature, shape};
use super::*;
use porter_core::{AccountId, Locality, Tier, Tokens};
use porter_infer::Slot;

fn engine(id: &str, url: &str, locality: Locality) -> Engine {
    Engine::new(
        EngineId::parse(id).expect("id"),
        AccountId::parse(id).expect("account"),
        "llama3.2:3b",
        EngineUrl::parse(url).expect("url"),
        locality,
        Dialect::LlamaServer,
        Tokens(1024),
    )
    .expect("engine")
}

fn row(engines: &[&str]) -> Route {
    Route {
        slot: Slot::Text,
        tier: Tier::Balanced,
        engines: engines
            .iter()
            .map(|id| EngineId::parse(id).expect("id"))
            .collect(),
    }
}

#[test]
fn engine_addresses_parse_to_scheme_host_port_and_base() {
    let cases = [
        (
            "http://127.0.0.1:11434/v1",
            Some((Scheme::Http, "127.0.0.1", 11434, "/v1")),
        ),
        (
            "https://api.openai.com/v1/",
            Some((Scheme::Https, "api.openai.com", 443, "/v1")),
        ),
        (
            "http://localhost",
            Some((Scheme::Http, "localhost", 80, "")),
        ),
        (
            "https://openrouter.ai/api/v1",
            Some((Scheme::Https, "openrouter.ai", 443, "/api/v1")),
        ),
        ("ftp://host/v1", None),
        ("127.0.0.1:11434/v1", None),
        ("http:///v1", None),
        ("http://host:notaport/v1", None),
        ("http://user@host/v1", None),
        ("http://host/v1?key=x", None),
    ];
    for (text, want) in cases {
        let got = EngineUrl::parse(text)
            .ok()
            .map(|url| (url.scheme, url.host, url.port, url.base));
        let want =
            want.map(|(scheme, host, port, base)| (scheme, host.to_owned(), port, base.to_owned()));
        assert_eq!(got, want, "{text}");
    }
}

#[test]
fn engine_ids_follow_the_id_grammar() {
    assert!(EngineId::parse("ollama-1").is_ok());
    for bad in ["", "Ollama", "-x", "a b"] {
        assert_eq!(
            EngineId::parse(bad),
            Err(EngineError::Id(bad.to_owned())),
            "{bad:?}"
        );
    }
}

#[test]
fn the_served_name_becomes_porters_model_id() {
    let engine = engine("ollama", "http://127.0.0.1:11434/v1", Locality::OnDevice);
    assert_eq!(engine.model.as_str(), "llama3.2-3b");
    assert_eq!(engine.served_name, "llama3.2:3b");
    let nameless = Engine::new(
        EngineId::parse("x").expect("id"),
        AccountId::parse("x").expect("account"),
        "///",
        EngineUrl::parse("http://127.0.0.1:1/v1").expect("url"),
        Locality::OnDevice,
        Dialect::LlamaServer,
        Tokens(1),
    );
    assert_eq!(nameless, Err(EngineError::Model("///".to_owned())));
}

#[test]
fn a_table_is_refused_for_what_would_go_wrong_later() {
    let local = || engine("local", "http://127.0.0.1:1/v1", Locality::OnDevice);
    let lan = || engine("lan", "http://192.168.1.5:1/v1", Locality::LocalNetwork).with_key();
    let secure = || {
        engine(
            "hosted",
            "https://api.example.com/v1",
            Locality::Cloud { region: None },
        )
        .with_key()
    };
    let loopback_key = || engine("keyed", "http://localhost:1/v1", Locality::OnDevice).with_key();
    let id = |text: &str| EngineId::parse(text).expect("id");

    assert!(
        Table::new(
            vec![local(), secure(), loopback_key()],
            vec![row(&["local", "hosted"])]
        )
        .is_ok()
    );
    assert_eq!(
        Table::new(vec![local(), local()], vec![]).err(),
        Some(EngineError::Duplicate(id("local")))
    );
    assert_eq!(
        Table::new(vec![local()], vec![row(&["local", "ghost"])]).err(),
        Some(EngineError::Unknown(id("ghost")))
    );
    // A key over plain http is only for this computer.
    assert_eq!(
        Table::new(vec![lan()], vec![]).err(),
        Some(EngineError::KeyInTheClear(id("lan")))
    );
}

#[test]
fn an_unset_sampling_leaves_the_temperature_to_the_engine() {
    let body = r#"{"model":"m","temperature":1.0,"stream":true}"#;
    let kept = shape(Temperature::Sent, body);
    assert_eq!(kept, body);
    let shaped: serde_json::Value =
        serde_json::from_str(&shape(Temperature::EngineDefault, body)).expect("json");
    assert_eq!(shaped, serde_json::json!({"model":"m","stream":true}));
    // Not an object: as it came.
    assert_eq!(shape(Temperature::EngineDefault, "[1]"), "[1]");
    assert_eq!(shape(Temperature::EngineDefault, "nope"), "nope");
}
