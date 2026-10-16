//! `Inference1.LocalEngines` on a private bus: which engine kinds inferd found a program for. The
//! fixed system paths are a list in scratch here, never this computer's `/usr/bin`.

use crate::hosting::bus::{PrivateBus, within};

use inferd::audit::Memory;
use inferd::clock::FixedClock;
use inferd::config::{Dirs, InferdConfig, ProgramSearch};
use inferd::engines::Engines;
use inferd::install::{EngineInstalls, HelperProgram, Parts};
use inferd::peers::{Caller, Role, TablePeers};
use inferd::service::{Inference, serve_on};
use inferd::settings::LocalEngines;
use inferd::supervise::Supervised;
use porter_core::{AppId, AppName, Isolation, UnixSeconds};
use porter_dbus::{EnginePlace, EngineState, InferenceProxy, LocalEngine, local_engine_from_dbus};
use porter_engines::{Arch, BackendChoice, Gpus, Host, Manifest, OsRelease, Roots};
use porter_engines_install::Fetcher;
use porter_engines_install::testkit::{release, row};
use porter_fake_servers::{start_artifacts, tls};
use porter_http::HyperHttp;
use porter_infer::{Policy, TierMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, ..) => name.to_string(),
        other => format!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_installs_an_engine_for_the_person_over_the_bus_and_nobody_else_may() {
    let scratch = porter_fake::Scratch::new("le-install");
    let host = start_artifacts().await.expect("host");
    let made = release(scratch.path(), "fixture", "");
    host.put("/e.tar.gz", made.archive.clone());
    let pinned = row(
        &host.url("/e.tar.gz"),
        &made,
        &made.seal,
        made.archive.len() as u64,
    );
    let installs = EngineInstalls::new(Parts {
        roots: Roots::new(scratch.path().join("system"), scratch.path().join("person")),
        manifest: Manifest::of(Box::leak(vec![pinned].into_boxed_slice())),
        host: Host {
            arch: Arch::X86_64,
            gpus: Gpus::default(),
            release: OsRelease::default(),
            choice: BackendChoice::Detect,
        },
        fetcher: Fetcher::new(HyperHttp::with_extra_roots(vec![tls::ca_der().to_vec()]))
            .with_timing(Duration::from_secs(5), 1, Duration::from_millis(10)),
        helper: HelperProgram {
            pkexec: PathBuf::from("/nonexistent/pkexec"),
            program: PathBuf::from("/nonexistent/helper"),
        },
        strata_folder: Arc::new(|| None),
    });
    let bus = PrivateBus::start();
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
    .engines_found(LocalEngines::of(&Default::default()).with_installs(installs.clone()))
    .engine_installs(installs);
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

    let denied = InferenceProxy::new(&app)
        .await
        .expect("proxy")
        .install_engine("llama_cpp", "just_me")
        .await;
    assert!(error_name(&denied.expect_err("refused")).ends_with("AccessDenied"));

    let proxy = InferenceProxy::new(&settings).await.expect("proxy");
    let not = |engine: &'static str, scope: &'static str| {
        let proxy = proxy.clone();
        async move { proxy.install_engine(engine, scope).await }
    };
    assert!(error_name(&not("nope", "just_me").await.expect_err("")).ends_with("InvalidArgs"));
    assert!(
        error_name(&not("llama_cpp", "everyone").await.expect_err("")).ends_with("InvalidArgs")
    );
    assert!(error_name(&not("vllm", "just_me").await.expect_err("")).ends_with("NotSupported"));
    // Without the helper there is nothing to install for everyone.
    assert!(
        error_name(&not("llama_cpp", "all_users").await.expect_err("")).ends_with("NotSupported")
    );

    let rows = proxy.local_engines().await.expect("rows");
    let llama = |rows: Vec<(String, porter_dbus::Details)>| {
        rows.into_iter()
            .map(|row| local_engine_from_dbus(row).expect("row"))
            .find(|engine| engine.id == "llama_cpp")
            .expect("llama.cpp")
    };
    let before = llama(rows);
    assert_eq!(before.state, EngineState::Missing);
    let offer = before.offer.expect("an offer");
    assert_eq!(offer.scopes, ["just_me"]);

    proxy
        .install_engine("llama_cpp", "just_me")
        .await
        .expect("started");
    let mut now = llama(proxy.local_engines().await.expect("rows"));
    for _ in 0..1000 {
        if matches!(now.state, EngineState::Found(_)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        now = llama(proxy.local_engines().await.expect("rows"));
    }
    assert!(matches!(now.state, EngineState::Found(_)), "{now:?}");
    assert_eq!(
        (now.place, now.version.as_deref()),
        (Some(EnginePlace::Person), Some("b-test"))
    );
    proxy
        .cancel_engine_install("llama_cpp")
        .await
        .expect("nothing to stop is not an error");
}
