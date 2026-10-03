//! The world of one test.

use super::bus::PrivateBus;
use super::engine::{FakeEngine, Script};
use engine_supervisor::{
    EngineHost, EngineId, ExitCode, FakeGpu, FakeReadyProbe, GpuMemory, HostError, Probe,
    SupervisorConfig, UnitSpec,
};
use inferd::audit::Memory;
use inferd::catalog::{CatalogDirs, EmbedSpec, read_catalog};
use inferd::clock::FixedClock;
use inferd::engines::Engines;
use inferd::local::{EngineConfig, LocalModel, build};
use inferd::peers::{Caller, Role, TablePeers};
use inferd::service::{Inference, serve_on};
use inferd::supervise::{Ports, Supervised};
use model_catalog::MiB;
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

/// What a test sets up.
pub struct Plan {
    /// Catalog files: (file name, text).
    pub catalog: Vec<(&'static str, String)>,
    /// What each model's engine says, by catalog id; a model with no script has no engine
    /// listening on its socket.
    pub scripts: Vec<(&'static str, Script)>,
    /// What the catalog does not say about embedding models.
    pub embeds: Vec<EmbedSpec>,
    /// Models of accounts that are not on this computer.
    pub remote: Vec<ModelCard>,
    /// The policy in force.
    pub policy: Policy,
    /// The role of the client connection.
    pub role: Role,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            catalog: Vec::new(),
            scripts: Vec::new(),
            embeds: Vec::new(),
            remote: Vec::new(),
            policy: Policy::proposed(),
            role: Role::App,
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
        };
        let sockets = scratch.join("s");
        std::fs::create_dir_all(&sockets).expect("sockets dir");
        let models = build(&catalog.entries, &plan.embeds, &engines_config, &sockets);
        for model in &models {
            // The weights are "in the cache": the directory the sandbox binds exists.
            let repo = model.spec.unit.sandbox.read.first().expect("a bind");
            std::fs::create_dir_all(repo).expect("weights dir");
        }
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
                host: host.clone(),
                probe: FakeReadyProbe(Probe::Ready),
                gpu: FakeGpu(GpuMemory {
                    total: MiB(16_000),
                    used_by_others: MiB(0),
                }),
            },
        );
        let served = Engines::new(models.clone(), supervised.clone(), plan.policy, TierMap::default())
            .with_remote(plan.remote);
        let bus = PrivateBus::start();
        let daemon = bus.connect().await;
        let client = bus.connect().await;
        let peers = Arc::new(TablePeers::new());
        peers.introduce(
            client.unique_name().expect("unique name").as_str(),
            Caller {
                app: test_app(),
                role: plan.role,
            },
        );
        let audit = Memory::default();
        serve_on(
            &daemon,
            Inference::new(
                served,
                peers,
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
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
