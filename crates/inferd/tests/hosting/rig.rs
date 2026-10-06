//! The world of one test.

use super::accountd::{FakeAccount, FakeAccountd};
use super::bus::PrivateBus;
use super::cloud::FakeCloud;
use super::engine::{FakeEngine, Script};
use engine_supervisor::{
    EngineHost, EngineId, ExitCode, FakeGpu, FakeReadyProbe, GpuMemory, HostError, Probe,
    SupervisorConfig, UnitSpec,
};
use inferd::audit::Memory;
use inferd::catalog::{CatalogDirs, read_catalog};
use inferd::clock::FixedClock;
use inferd::cloud::Cloud;
use inferd::cloud::accountd::PeerAccountd;
use inferd::cloud::spend::Ledger;
use inferd::cloud::wire::{Door, Doors};
use inferd::engines::Engines;
use inferd::local::{EngineConfig, LocalModel, build};
use inferd::peers::{Caller, Role, TablePeers};
use inferd::replay::{NamedEngine, Replays};
use inferd::service::{Inference, serve_on};
use inferd::settings::{Settings, SpendLine};
use inferd::supervise::{Ports, Supervised};
use model_catalog::MiB;
use model_http::{DerCertificate, TlsRoots};
use porter_client::{Accounts, DbusTransport};
use porter_core::{AppId, AppName, Isolation, UnixSeconds};
use porter_infer::{ModelCard, Policy, TierMap};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// An engine host that starts nothing: it records what it was asked for and its engines never
/// exit.
#[derive(Debug, Clone, Default)]
pub struct Recorder {
    pub spawned: Arc<Mutex<Vec<EngineId>>>,
}

impl EngineHost for Recorder {
    async fn spawn(&self, id: &EngineId, _: &UnitSpec) -> Result<(), HostError> {
        self.spawned.lock().expect("lock").push(id.clone());
        Ok(())
    }

    async fn stop(&self, _: &EngineId) -> Result<(), HostError> {
        Ok(())
    }

    async fn exited(&self, _: &EngineId) -> ExitCode {
        std::future::pending().await
    }
}

/// Whom a daemon trusts to be a hosted model's server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The scratch CA the fake provider's certificate is signed by.
    ScratchCa,
    /// No one: every handshake fails.
    NoOne,
}

/// The hosted side of a test: the accounts the fake accountd holds and how the fake provider
/// answers. Every provider the shipped catalogue reaches is the one fake, at its own base path.
pub struct Hosted {
    pub accounts: Vec<FakeAccount>,
    pub chat: Script,
    pub trust: Trust,
}

/// What a test sets up.
pub struct Plan {
    /// Catalog files: (file name, text).
    pub catalog: Vec<(&'static str, String)>,
    /// What each model's engine says, by catalog id; a model with no script has no engine
    /// listening on its socket.
    pub scripts: Vec<(&'static str, Script)>,
    /// Replay engines: (name, cassette file), as `[engines.<name>] replay = ...` would say.
    pub replay: Vec<(&'static str, PathBuf)>,
    /// Models of accounts that are not on this computer.
    pub remote: Vec<ModelCard>,
    /// The policy in force.
    pub policy: Policy,
    /// The role of the client connection.
    pub role: Role,
    /// The app the client connection is (`test_app()` when none).
    pub app: Option<AppId>,
    /// Hosted models: a fake accountd and a fake provider; none when absent.
    pub hosted: Option<Hosted>,
    /// The spend rows in force.
    pub spend: SpendLine,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            catalog: Vec::new(),
            scripts: Vec::new(),
            replay: Vec::new(),
            remote: Vec::new(),
            policy: Policy::proposed(),
            role: Role::App,
            app: None,
            hosted: None,
            spend: SpendLine::default(),
        }
    }
}

/// A running daemon and a client connected to it.
pub struct World {
    pub bus: PrivateBus,
    pub scratch: PathBuf,
    pub daemon: zbus::Connection,
    pub client: zbus::Connection,
    pub accounts: Accounts<DbusTransport>,
    pub engines: BTreeMap<String, FakeEngine>,
    pub models: Vec<LocalModel>,
    pub audit: Memory,
    pub host: Recorder,
    pub supervised: Supervised,
    /// The engines the daemon routes through (its cloud, ledger and settings are reachable).
    pub served: Engines,
    /// The fake accountd, when the plan has hosted models.
    pub accountd: Option<FakeAccountd>,
    /// The fake provider, when it has.
    pub provider: Option<FakeCloud>,
    /// Who the daemon knows the bus connections as; a test introduces more.
    pub peers: Arc<TablePeers>,
    /// The connection the fake accountd is served on (it is gone with this).
    pub accountd_bus: Option<zbus::Connection>,
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// The app every test client is.
pub fn test_app() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Test").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

impl World {
    pub async fn start(plan: Plan) -> World {
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "inf-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let catalog_dir = scratch.join("cat");
        std::fs::create_dir_all(&catalog_dir).expect("catalog dir");
        for (name, text) in &plan.catalog {
            std::fs::write(catalog_dir.join(name), text.as_bytes()).expect("catalog file");
        }
        let catalog = read_catalog(&CatalogDirs {
            system: catalog_dir,
            user: scratch.join("none"),
        });
        assert_eq!(catalog.skipped, vec![], "every test entry parses");
        let engines_config = EngineConfig {
            vllm_python: Some(PathBuf::from("/nonexistent/python")),
            llama_server: Some(PathBuf::from("/nonexistent/llama-server")),
            speech_host: None,
            kokoro_python: None,
            hf_cache: scratch.join("hf"),
            ..EngineConfig::default()
        };
        let sockets = scratch.join("s");
        std::fs::create_dir_all(&sockets).expect("sockets dir");
        let mut models = build(&catalog.entries, &engines_config, &sockets);
        for model in &models {
            // The weights are "in the cache": the directory the sandbox binds exists.
            let repo = model.spec.unit.sandbox.read.first().expect("a bind");
            std::fs::create_dir_all(repo).expect("weights dir");
        }
        let named = plan
            .replay
            .iter()
            .map(|(name, file)| {
                (
                    (*name).to_owned(),
                    NamedEngine {
                        replay: file.clone(),
                        record: None,
                    },
                )
            })
            .collect();
        let replays = Replays::read(&named, &sockets);
        models.extend(replays.models.iter().cloned());
        let engines: BTreeMap<String, FakeEngine> = plan
            .scripts
            .into_iter()
            .map(|(id, script)| {
                let model = models
                    .iter()
                    .find(|m| m.entry.id.0 == id)
                    .unwrap_or_else(|| panic!("no model {id}"));
                (id.to_owned(), FakeEngine::start(&model.socket.0, script))
            })
            .collect();
        let host = Recorder::default();
        let supervised = Supervised::start(
            models.iter().map(|m| m.spec.clone()).collect(),
            SupervisorConfig::default(),
            Ports {
                host: replays.host(host.clone()),
                probe: FakeReadyProbe(Probe::Ready),
                gpu: FakeGpu(GpuMemory {
                    total: MiB(16_000),
                    used_by_others: MiB(0),
                }),
            },
        );
        let mut served = Engines::new(
            models.clone(),
            supervised.clone(),
            plan.policy.clone(),
            TierMap::default(),
        )
        .with_settings(Settings {
            policy: plan.policy,
            spend: plan.spend,
            ..Settings::default()
        })
        .with_remote(plan.remote);
        let bus = PrivateBus::start();
        let daemon = bus.connect().await;
        let client = bus.connect().await;
        let (mut accountd, mut provider, mut accountd_bus) = (None, None, None);
        if let Some(hosted) = plan.hosted {
            let fake = FakeAccountd::new(hosted.accounts);
            let held = bus.connect().await;
            fake.serve(&held).await;
            accountd_bus = Some(held);
            let api = FakeCloud::start(hosted.chat).await;
            let door = |base: &str| Door {
                host: "localhost".to_owned(),
                port: api.port,
                base: base.to_owned(),
            };
            let doors = Doors::real()
                .with("openrouter", door("/api/v1"))
                .with("openai", door("/v1"))
                .with("moonshot", door("/moonshot/v1"))
                .with("google-ai", door("/gemini/v1beta/openai"));
            let roots = match hosted.trust {
                Trust::ScratchCa => TlsRoots::Only(vec![DerCertificate(
                    porter_fake_servers::tls::ca_der().to_vec(),
                )]),
                Trust::NoOne => TlsRoots::Only(Vec::new()),
            };
            let cloud = Cloud::new(
                Arc::new(PeerAccountd::new(daemon.clone())),
                &catalog.entries,
                Ledger::in_memory(),
                Arc::new(FixedClock(UnixSeconds(1_700_000_000))),
            )
            .at(doors, roots);
            served = served.with_cloud(cloud);
            (accountd, provider) = (Some(fake), Some(api));
        }
        let peers = Arc::new(TablePeers::new());
        peers.introduce(
            client.unique_name().expect("unique name").as_str(),
            Caller {
                app: plan.app.unwrap_or_else(test_app),
                role: plan.role,
            },
        );
        let audit = Memory::default();
        serve_on(
            &daemon,
            Inference::new(
                served.clone(),
                Arc::clone(&peers),
                audit.clone(),
                FixedClock(UnixSeconds(1_700_000_000)),
            ),
        )
        .await
        .expect("serve Inference1");
        World {
            accounts: Accounts::over(DbusTransport::over(client.clone())),
            bus,
            scratch,
            daemon,
            client,
            engines,
            models,
            audit,
            host,
            supervised,
            served,
            accountd,
            provider,
            peers,
            accountd_bus,
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
