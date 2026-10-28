//! `Inference1.LocalEngines` on a private bus: which engine kinds inferd found a program for. The
//! fixed system paths are a list in scratch here, never this computer's `/usr/bin`.

use crate::hosting::bus::{PrivateBus, within};

use engine_manifest::{Arch, Artifact, Backend, Delivery, Manifest, Row};
use engine_probe::{Record, Roots, Scope, seal_of};
use inferd::config::{Dirs, InferdConfig, ProgramSearch};
use inferd::engines::Engines;
use inferd::installed::Installed;
use inferd::peers::{Caller, Role, TablePeers};
use inferd::service::{Inference, serve_on};
use inferd::settings::LocalEngines;
use inferd::settings::{ConfigFile, Reload};
use inferd::supervise::Supervised;
use model_catalog::EngineKind;
use porter_core::{AppId, AppName, Isolation};
use porter_dbus::inference::{
    EnginePlace, EngineState, InferenceProxy, LocalEngine, local_engine_from_dbus,
};
use porter_ids::UnixSeconds;
use porter_ids::clock::FixedClock;
use porter_infer::{Policy, TierMap};
use porter_turns::audit::Memory;
use std::os::unix::fs::PermissionsExt;
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
    let scratch = test_kit::Scratch::new("le-named");
    let server = scratch.path().join("llama-server");
    std::fs::write(&server, b"").expect("program");
    let file = format!("[engines]\nllama_server = {:?}\n", server.to_string_lossy());
    let engines = asked(Role::Settings, &file, Vec::new()).await.expect("ask");
    assert_eq!(engines.len(), 5);
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
    let scratch = test_kit::Scratch::new("le-fixed");
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
    let scratch = test_kit::Scratch::new("le-missing");
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

static NO_FILES: [Artifact; 0] = [];

/// An install of llama.cpp for the person made by hand under `roots`: its tree, its seal and its
/// record, and the manifest that pins it.
fn made_install(roots: &Roots) -> (Manifest, PathBuf) {
    let dir = roots
        .engine_dir(Scope::JustMe, EngineKind::LlamaServer)
        .join("b-test-cpu");
    let tree = dir.join("engine");
    std::fs::create_dir_all(&tree).expect("tree");
    let program = tree.join("llama-server");
    std::fs::write(&program, "#!/bin/sh\n").expect("program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let seal = seal_of(&tree).expect("seal").as_hex().to_owned();
    Record {
        engine: "llama_cpp".into(),
        version: "b-test".into(),
        backend: "cpu".into(),
        seal: seal.clone(),
        entries: Default::default(),
        venv_seal: None,
    }
    .write(&dir.join("installed.json"))
    .expect("record");
    let row = Row {
        engine: EngineKind::LlamaServer,
        arch: Arch::X86_64,
        backend: Backend::Cpu,
        version: "b-test",
        min_nvidia_driver: None,
        delivery: Delivery::Archives(&NO_FILES),
        program: "llama-server",
        tree_seal: Some(Box::leak(seal.into_boxed_str())),
    };
    (
        Manifest::of(Box::leak(vec![row].into_boxed_slice())),
        program,
    )
}

fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, ..) => name.to_string(),
        other => format!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_install_of_ours_says_its_release_and_place_and_a_changed_one_says_so_and_strata_is_pointed_at()
 {
    let scratch = test_kit::Scratch::new("le-installed");
    let roots = Roots::new(scratch.path().join("system"), scratch.path().join("person"));
    let (manifest, program) = made_install(&roots);
    let file = scratch.path().join("inferd.toml");
    let installed = Installed::new(roots, manifest, |_| true, {
        let file = ConfigFile::new(file.clone());
        Arc::new(move || {
            file.read()
                .ok()
                .and_then(|config| config.engines.strata_folder)
        })
    });
    let bus = PrivateBus::start();
    let peers = Arc::new(TablePeers::new());
    let engines = Engines::new(
        Vec::new(),
        Supervised::idle(),
        Policy::proposed(),
        TierMap::default(),
    );
    let reload = Reload::new(ConfigFile::new(file.clone()), engines.clone());
    let inference = Inference::new(
        engines,
        Arc::clone(&peers),
        Memory::default(),
        FixedClock(UnixSeconds(1_700_000_000)),
    )
    .reloading(reload)
    .engines_found(LocalEngines::of(&Default::default()).with_installed(installed.clone()));
    let daemon = bus.connect().await;
    within("inferd to serve", serve_on(&daemon, inference))
        .await
        .expect("serve");
    let settings = bus.connect().await;
    peers.introduce(
        settings.unique_name().expect("name").as_str(),
        who(Role::Settings),
    );
    let app = bus.connect().await;
    peers.introduce(app.unique_name().expect("name").as_str(), who(Role::App));
    let proxy = InferenceProxy::new(&settings).await.expect("proxy");
    let row_of = |id: &'static str| {
        let proxy = proxy.clone();
        async move {
            proxy
                .local_engines()
                .await
                .expect("rows")
                .into_iter()
                .map(|row| local_engine_from_dbus(row).expect("row"))
                .find(|engine| engine.id == id)
                .expect("listed")
        }
    };
    let llama = row_of("llama_cpp").await;
    assert_eq!(llama.state, EngineState::Found(program.clone()));
    assert_eq!(
        (llama.place, llama.version.as_deref()),
        (Some(EnginePlace::Person), Some("b-test"))
    );
    // A tree that is not the pinned one is not started, and the row says it was changed.
    let guard = installed.guard();
    std::fs::write(program.with_file_name("libplanted.so"), "x").expect("plant");
    assert!(guard.check(&program).is_err());
    assert_eq!(
        row_of("llama_cpp").await.state,
        EngineState::Changed(program)
    );

    // Strata: counted once the person points at a folder that holds it; nobody else may.
    let folder = scratch.path().join("strata");
    std::fs::create_dir_all(folder.join("serve")).expect("serve");
    std::fs::write(folder.join("serve/server.py"), "").expect("server");
    let python = folder.join(".venv/bin/python");
    std::fs::create_dir_all(python.parent().expect("parent")).expect("bin");
    std::fs::write(&python, "").expect("python");
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let folder_text = folder.to_string_lossy().into_owned();
    let denied = InferenceProxy::new(&app)
        .await
        .expect("proxy")
        .point_engine("strata", &folder_text)
        .await;
    assert!(error_name(&denied.expect_err("refused")).ends_with("AccessDenied"));
    for (engine, place) in [
        ("strata", "/nonexistent"),
        ("llama_cpp", folder_text.as_str()),
    ] {
        let refused = proxy.point_engine(engine, place).await;
        assert!(
            error_name(&refused.expect_err("refused")).ends_with("InvalidArgs"),
            "{engine}"
        );
    }
    assert_eq!(row_of("strata").await.state, EngineState::Missing);
    proxy
        .point_engine("strata", &folder_text)
        .await
        .expect("pointed");
    assert_eq!(row_of("strata").await.state, EngineState::Found(python));
}
