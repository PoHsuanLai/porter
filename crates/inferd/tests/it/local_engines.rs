//! `Inference1.LocalEngines` on a private bus: which engine kinds inferd found a program for. The
//! fixed system paths are a list in scratch here, never this computer's `/usr/bin`.

use crate::hosting::bus::{PrivateBus, within};

use inferd::audit::Memory;
use inferd::clock::FixedClock;
use inferd::config::{Dirs, InferdConfig, ProgramSearch};
use inferd::engines::Engines;
use inferd::peers::{Caller, Role, TablePeers};
use inferd::service::{Inference, serve_on};
use inferd::settings::LocalEngines;
use inferd::supervise::Supervised;
use porter_core::{AppId, AppName, Isolation, UnixSeconds};
use porter_dbus::{EngineState, InferenceProxy, LocalEngine, local_engine_from_dbus};
use porter_infer::{Policy, TierMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn who(role: Role) -> Caller {
    Caller {
        app: AppId {
            name: AppName::parse("org.quire.Client").expect("app name"),
            isolation: Isolation::Unsandboxed,
        },
        role,
    }
}

fn dirs(home: &Path) -> Dirs {
    let home = home.to_string_lossy().into_owned();
    Dirs::from_vars(|name| match name {
        "HOME" => Some(home.clone()),
        "XDG_RUNTIME_DIR" => Some(format!("{home}/run")),
        _ => None,
    })
    .expect("dirs")
}

/// What `LocalEngines` answers to a caller that is `role`, over `file` as `inferd.toml` and the
/// fixed paths `fixed`.
async fn asked(role: Role, file: &str, fixed: Vec<PathBuf>) -> zbus::Result<Vec<LocalEngine>> {
    let bus = PrivateBus::start();
    let config = InferdConfig::from_toml(file).expect("config");
    let programs = ProgramSearch::in_paths(fixed, |path| path.exists());
    let found = config.engines_with(&dirs(bus.scratch()), &programs);
    let peers = Arc::new(TablePeers::new());
    let engines = Engines::new(
        Vec::new(),
        Supervised::idle(),
        Policy::proposed(),
        TierMap::default(),
    );
    let inference = Inference::new(
        engines,
        Arc::clone(&peers),
        Memory::default(),
        FixedClock(UnixSeconds(1_700_000_000)),
    )
    .engines_found(LocalEngines::of(&found));
    let daemon = bus.connect().await;
    within("inferd to serve", serve_on(&daemon, inference))
        .await
        .expect("serve");
    let client = bus.connect().await;
    peers.introduce(client.unique_name().expect("name").as_str(), who(role));
    let rows = InferenceProxy::new(&client).await?.local_engines().await?;
    Ok(rows
        .into_iter()
        .map(|row| local_engine_from_dbus(row).expect("a row as the interface says"))
        .collect())
}

fn llama(engines: &[LocalEngine]) -> &LocalEngine {
    engines
        .iter()
        .find(|engine| engine.name == "llama.cpp")
        .expect("llama.cpp is listed")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_named_in_the_file_is_found_with_its_path() {
    let scratch = porter_fake::Scratch::new("le-named");
    let server = scratch.path().join("llama-server");
    std::fs::write(&server, b"").expect("program");
    let file = format!("[engines]\nllama_server = {:?}\n", server.to_string_lossy());
    let engines = asked(Role::Settings, &file, Vec::new()).await.expect("ask");
    assert_eq!(engines.len(), 4);
    assert_eq!(llama(&engines).state, EngineState::Found(server));
    assert!(llama(&engines).slots.contains(&"text".to_owned()));
    assert!(
        engines
            .iter()
            .filter(|engine| engine.name != "llama.cpp")
            .all(|engine| engine.state == EngineState::Missing)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_at_a_fixed_path_is_found_when_the_file_names_none() {
    let scratch = porter_fake::Scratch::new("le-fixed");
    let server = scratch.path().join("llama-server");
    std::fs::write(&server, b"").expect("program");
    let engines = asked(
        Role::Shell,
        "",
        vec![scratch.path().join("gone"), server.clone()],
    )
    .await
    .expect("ask");
    assert_eq!(llama(&engines).state, EngineState::Found(server));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_path_or_a_path_that_is_not_there_is_missing() {
    let scratch = porter_fake::Scratch::new("le-missing");
    let gone = scratch.path().join("gone");
    let none = asked(Role::Settings, "", vec![gone.clone()])
        .await
        .expect("ask");
    assert_eq!(llama(&none).state, EngineState::Missing);
    let file = format!("[engines]\nllama_server = {:?}\n", gone.to_string_lossy());
    let named = asked(Role::Settings, &file, Vec::new()).await.expect("ask");
    assert_eq!(llama(&named).state, EngineState::Missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_may_not_ask() {
    let refused = asked(Role::App, "", Vec::new()).await;
    assert!(
        matches!(refused, Err(zbus::Error::MethodError(..))),
        "{refused:?}"
    );
}
