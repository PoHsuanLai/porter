//! The world of one test.

use super::accountd::{FakeAccount, FakeAccountd};
use super::bus::{PrivateBus, within};
use super::cloud::FakeCloud;
use super::engine::{FakeEngine, Script};
use super::speech_host::SpeechEngines;
use super::tailnet::{FakeTailscale, TailnetPlan};
use engine_supervisor::{
    EngineHost, EngineId, ExitCode, FakeGpu, GpuMemory, HostError, Probe, ReadyProbe,
    SupervisorConfig, UnitSpec,
};
use inferd::attached::{Attached, AttachedBook};
use inferd::audit::Memory;
use inferd::catalog::{CatalogDirs, read_catalog};
use inferd::clock::FixedClock;
use inferd::cloud::Cloud;
use inferd::cloud::accountd::PeerAccountd;
use inferd::cloud::spend::Ledger;
use inferd::cloud::wire::{Door, Doors};
use inferd::engines::Engines;
use inferd::hosts::{HealthProbe, ProcessHost};
use inferd::local::{EngineConfig, LocalModel, build};
use inferd::peers::{Caller, Role, TablePeers};
use inferd::probe::ProbeConfig;
use inferd::replay::{NamedEngine, Replays};
use inferd::report::PeerReports;
use inferd::service::{Inference, serve_on};
use inferd::settings::{Settings, SpendLine, TailnetServe};
use inferd::startup::{Level, Log};
use inferd::supervise::{Ports, Supervised};
use inferd::tailnet::{BusMachines, Parts, Tailnet};
use inferd::watch::{Probing, Watch};
use model_catalog::MiB;
use model_http::{DerCertificate, TlsRoots};
use porter_client::{Accounts, DbusTransport};
use porter_core::{AppId, AppName, Isolation, UnixSeconds};
use porter_infer::{ModelCard, Policy, TierMap};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// An engine host that starts nothing: it records what it was asked for. An engine never exits by
/// itself; one that is stopped exits (an eviction or an idle unload completes).
#[derive(Debug, Clone, Default)]
pub struct Recorder {
    pub spawned: Arc<Mutex<Vec<EngineId>>>,
    pub stopped: Arc<Mutex<Vec<EngineId>>>,
    gone: Arc<tokio::sync::Notify>,
}

impl EngineHost for Recorder {
    async fn spawn(&self, id: &EngineId, _: &UnitSpec) -> Result<(), HostError> {
        self.stopped.lock().expect("lock").retain(|one| one != id);
        self.spawned.lock().expect("lock").push(id.clone());
        Ok(())
    }

    async fn stop(&self, id: &EngineId) -> Result<(), HostError> {
        self.stopped.lock().expect("lock").push(id.clone());
        self.gone.notify_waiters();
        Ok(())
    }

    async fn exited(&self, id: &EngineId) -> ExitCode {
        loop {
            let woken = self.gone.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            if self.stopped.lock().expect("lock").contains(id) {
                return ExitCode(0);
            }
            woken.await;
        }
    }
}

/// Where the fake speech host's program "is" (it is never run: the fake engine host serves the
/// protocol in place of it).
pub const SPEECH_HOST_PROGRAM: &str = "/fake/speech-host";

/// A speech host in the world: what it says, and the libraries its unit is told about.
#[derive(Debug, Clone)]
pub struct SpeechPlan {
    /// The words each utterance is heard as.
    pub words: Vec<&'static str>,
    /// `engines.speech_host_libs`.
    pub libs: Option<PathBuf>,
}

/// An engine host with two halves: the speech engine's program is the fake speech host, anything
/// else is recorded and starts nothing.
#[derive(Debug, Clone)]
struct Hub {
    recorder: Recorder,
    speech: SpeechEngines,
}

impl EngineHost for Hub {
    async fn spawn(&self, id: &EngineId, unit: &UnitSpec) -> Result<(), HostError> {
        if unit.program.0 == std::path::Path::new(SPEECH_HOST_PROGRAM) {
            self.speech.spawn(id, unit).await
        } else {
            self.recorder.spawn(id, unit).await
        }
    }

    async fn stop(&self, id: &EngineId) -> Result<(), HostError> {
        match self.speech.stop(id).await {
            Err(HostError::NotRunning) => self.recorder.stop(id).await,
            done => done,
        }
    }

    async fn exited(&self, id: &EngineId) -> ExitCode {
        if self
            .speech
            .units
            .lock()
            .expect("lock")
            .iter()
            .any(|(one, _)| one == id)
        {
            self.speech.exited(id).await
        } else {
            self.recorder.exited(id).await
        }
    }
}

/// The speech engines are asked `Hello` (the real probe over the real socket); every other engine
/// answers ready.
#[derive(Debug, Clone)]
struct Probes {
    speech: HealthProbe,
    ids: Vec<EngineId>,
}

impl ReadyProbe for Probes {
    async fn probe(&self, id: &EngineId) -> Probe {
        if self.ids.contains(id) {
            self.speech.probe(id).await
        } else {
            Probe::Ready
        }
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
    /// Which ports are probed for runtimes the person runs (the fake accountd hears the
    /// reports); none probes nothing and wires nothing.
    pub probe: Option<ProbeConfig>,
    /// A speech host (the fake one), when the plan has speech models.
    pub speech: Option<SpeechPlan>,
    /// The supervisor's timing, when a test needs a restart to take milliseconds.
    pub supervisor: Option<SupervisorConfig>,
    /// Models (catalog ids) whose language card also takes audio: the catalogue has no entry
    /// that says so yet, and a test needs a text model that hears.
    pub hears: Vec<&'static str>,
    /// Engines that are real child processes: llama-server's program is this one, run on the
    /// model's socket by the real process host (no recorder, no fake probe).
    pub processes: Option<Processes>,
    /// Engines the person already runs (`[engines.attached.<id>]`), each over a catalog entry of
    /// `catalog`; inferd looks at them and never starts them.
    pub attached: Vec<Attached>,
    /// The agent endpoints (`org.quire.Inference1.Agents`): served when present.
    pub agents: Option<AgentsPlan>,
    /// The runtime the daemon connection's object server is started on, when a test holds it
    /// back (zbus starts it on a task of its own, on the runtime of its first use); none starts
    /// it on the test's own.
    pub dispatcher_on: Option<tokio::runtime::Handle>,
    /// Whether this computer has the programs that run engines (llama-server, vLLM): a computer
    /// without them builds no model of the catalogue, and can only borrow.
    pub engine_programs: bool,
    /// This computer on a fake Tailscale network; none keeps inferd off the network altogether.
    pub tailnet: Option<TailnetPlan>,
}

/// The agent endpoints of a world: the setting, and who the launcher is.
#[derive(Debug, Clone, Copy)]
pub struct AgentsPlan {
    /// `ai.agents.endpoint`.
    pub endpoint: inferd::settings::AgentEndpoint,
}

/// The program a real-process world runs as llama-server (a script the test wrote).
#[derive(Debug, Clone)]
pub struct Processes {
    pub program: PathBuf,
}

/// The lines inferd logged in a real-process world.
pub type LogLines = Arc<Mutex<Vec<(Level, String)>>>;

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
            probe: None,
            speech: None,
            supervisor: None,
            hears: Vec::new(),
            processes: None,
            attached: Vec::new(),
            agents: None,
            dispatcher_on: None,
            engine_programs: true,
            tailnet: None,
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
    /// The look for local runtimes, when the plan has one (it stops with this).
    pub probing: Option<Probing>,
    /// The fake speech engines, when the plan has a speech host.
    pub speech: Option<SpeechEngines>,
    /// What inferd logged (kept only in a real-process world).
    pub log: LogLines,
    /// Where the computers Settings adds are kept (`computers.toml`, `computer-keys/`).
    pub state: PathBuf,
    /// This computer's side of the fake Tailscale network, when the plan has one.
    pub tailscale: Option<FakeTailscale>,
    /// inferd's side of that network.
    pub tailnet: Option<Tailnet>,
    /// The connection the fake `Tailnet1` is served on (it is gone with this).
    pub tailnet_bus: Option<zbus::Connection>,
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
            vllm_python: plan
                .engine_programs
                .then(|| PathBuf::from("/nonexistent/python")),
            llama_server: plan.engine_programs.then(|| {
                plan.processes.as_ref().map_or_else(
                    || PathBuf::from("/nonexistent/llama-server"),
                    |p| p.program.clone(),
                )
            }),
            speech_host: plan
                .speech
                .as_ref()
                .map(|_| PathBuf::from(SPEECH_HOST_PROGRAM)),
            speech_host_libs: plan.speech.as_ref().and_then(|speech| speech.libs.clone()),
            kokoro_python: None,
            hf_cache: scratch.join("hf"),
            ..EngineConfig::default()
        };
        let sockets = scratch.join("s");
        std::fs::create_dir_all(&sockets).expect("sockets dir");
        let mut models = build(&catalog.entries, &engines_config, &sockets);
        for model in &mut models {
            if plan.hears.contains(&model.entry.id.0.as_str()) {
                for capability in &mut model.card.capabilities {
                    if let porter_core::Capability::Llm(llm) = capability {
                        llm.features
                            .insert(porter_core::capability::LlmFeature::AudioIn);
                    }
                }
            }
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
        let attached = inferd::attached::models(&plan.attached, &catalog.entries, &sockets)
            .expect("the attached engines name catalog entries");
        models.retain(|model| {
            attached
                .iter()
                .all(|one| one.entry.id.0 != model.entry.id.0)
        });
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
        let speech = plan
            .speech
            .as_ref()
            .map(|speech| SpeechEngines::new(&speech.words));
        let speech_ids: Vec<EngineId> = models
            .iter()
            .filter(|m| m.profile.kind == model_catalog::EngineKind::SpeechHost)
            .map(|m| m.spec.id.clone())
            .collect();
        let probes = Probes {
            speech: HealthProbe::new(
                models
                    .iter()
                    .map(|m| (m.spec.id.clone(), m.socket.0.clone())),
            )
            .speech_hosts(speech_ids.clone()),
            ids: speech_ids,
        };
        let gpu = FakeGpu(GpuMemory {
            total: MiB(16_000),
            used_by_others: MiB(0),
        });
        let specs: Vec<_> = models.iter().map(|m| m.spec.clone()).collect();
        let timing = plan.supervisor.unwrap_or_default();
        let log: LogLines = Arc::default();
        let supervised = match &plan.processes {
            None => Supervised::start(
                specs,
                timing,
                Ports {
                    host: replays.host(Hub {
                        recorder: host.clone(),
                        speech: speech.clone().unwrap_or_else(|| SpeechEngines::new(&[])),
                    }),
                    probe: probes,
                    gpu,
                },
            ),
            // Real child processes (the scratch program), the real health probe over their
            // sockets, and inferd's log lines kept for the test to read.
            Some(_) => {
                let processes = ProcessHost::new().with_sockets(
                    models
                        .iter()
                        .map(|m| (m.spec.id.clone(), m.socket.0.clone())),
                );
                let sink = Arc::clone(&log);
                let diagnostics =
                    processes
                        .diagnostics()
                        .logging_to(Log::to(move |level, text| {
                            sink.lock().expect("lock").push((level, text.to_owned()));
                        }));
                let probe = HealthProbe::new(
                    models
                        .iter()
                        .map(|m| (m.spec.id.clone(), m.socket.0.clone())),
                );
                Supervised::start_with(
                    specs,
                    timing,
                    Ports {
                        host: replays.host(processes),
                        probe,
                        gpu,
                    },
                    diagnostics,
                )
            }
        };
        let mut served = Engines::new(
            models.clone(),
            supervised.clone(),
            plan.policy.clone(),
            TierMap::default(),
        )
        .with_settings(Settings {
            policy: plan.policy,
            spend: plan.spend,
            agent_endpoint: plan
                .agents
                .map_or_else(Default::default, |agents| agents.endpoint),
            tailnet_serve: match plan.tailnet.as_ref().is_some_and(|plan| plan.serving) {
                true => TailnetServe::On,
                false => TailnetServe::Off,
            },
            ..Settings::default()
        })
        .with_remote(plan.remote)
        .with_attached({
            let sink = Arc::clone(&log);
            AttachedBook::new(attached)
                .logging_to(Log::to(move |level, text| {
                    sink.lock().expect("lock").push((level, text.to_owned()));
                }))
                .with_catalogue(catalog.entries.clone(), sockets.clone())
        });
        // The computers Settings adds live in the world's own state directory.
        let state = scratch.join("state");
        let computers = inferd::attached::Computers::new(
            state.join("computers.toml"),
            state.join("computer-keys"),
            served.attached().clone(),
            inferd::attached::AddedFile::default(),
            &plan.attached,
        );
        let bus = PrivateBus::start();
        let daemon = bus.connect().await;
        let client = bus.connect().await;
        let (mut accountd, mut provider, mut accountd_bus) = (None, None, None);
        if let Some(hosted) = plan.hosted {
            let fake = FakeAccountd::new(hosted.accounts);
            let held = bus.connect().await;
            within("the fake accountd on the bus", fake.serve(&held)).await;
            accountd_bus = Some(held);
            let api = FakeCloud::start(hosted.chat).await;
            let door = |base: &str| Door {
                host: "localhost".to_owned(),
                port: api.port,
                base: base.to_owned(),
            };
            let doors = Doors::real()
                .with("anthropic", door("/v1"))
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
        let probing = plan.probe.map(|config| {
            Watch::new(
                porter_http::HyperHttp::new(),
                config,
                served.clone(),
                Arc::new(PeerReports::new(daemon.clone())),
                scratch.join("s"),
            )
            .spawn()
        });
        let mut inference = Inference::new(
            served.clone(),
            Arc::clone(&peers),
            audit.clone(),
            FixedClock(UnixSeconds(1_700_000_000)),
        );
        inference = inference.computers(computers);
        if let Some(probing) = &probing {
            inference = inference.probing(probing.clone());
        }
        if plan.agents.is_some() {
            inference = inference.agents(inferd::agent::Agents::new(
                served.clone(),
                Arc::new(audit.clone()),
                Arc::new(FixedClock(UnixSeconds(1_700_000_000))),
            ));
        }
        // This computer on a fake Tailscale network: its Tailscale, accountd's list of the
        // computers (on the connection accountd's fake is on, else on one that owns the name),
        // the answers it keeps, and inferd's side.
        let (tailscale, tailnet, tailnet_bus) = match plan.tailnet {
            None => (None, None, None),
            Some(tp) => {
                let tailscale = FakeTailscale::start(&scratch.join("ts"), tp.network).await;
                let held = match &accountd_bus {
                    Some(held) => held.clone(),
                    None => {
                        let held = bus.connect().await;
                        held.request_name("org.quire.Accounts1")
                            .await
                            .expect("own org.quire.Accounts1");
                        held
                    }
                };
                held.object_server()
                    .at(porter_dbus::TAILNET_PATH, tailscale.object())
                    .await
                    .expect("serve the fake Tailnet1");
                let (guests, said) = porter_tailnet::Guests::open(&state.join("guests.toml"));
                assert_eq!(said, None);
                let tailnet = Tailnet::start(
                    Parts {
                        api: tailscale.api.clone(),
                        machines: Arc::new(BusMachines::new(daemon.clone())),
                        guests: Arc::new(guests),
                        sockets: sockets.clone(),
                        config: porter_tailnet::Config {
                            port: tp.port,
                            allowed: |ip| {
                                ip.is_loopback() && ip != std::net::IpAddr::from([127, 0, 0, 1])
                            },
                            limits: porter_tailnet::Limits::default(),
                            first_pause: std::time::Duration::from_millis(50),
                            longest_pause: std::time::Duration::from_millis(200),
                        },
                        timing: porter_tailnet::Timing {
                            backstop: std::time::Duration::from_millis(300),
                            retry: std::time::Duration::from_millis(100),
                        },
                    },
                    served.clone(),
                    Arc::new(audit.clone()),
                    Arc::new(FixedClock(UnixSeconds(1_700_000_000))),
                );
                inference = inference.tailnet(tailnet.clone());
                (Some(tailscale), Some(tailnet), Some(held))
            }
        };
        if let Some(runtime) = &plan.dispatcher_on {
            let _on = runtime.enter();
            daemon.object_server();
        }
        within("Inference1 on the bus", serve_on(&daemon, inference))
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
            probing,
            speech,
            log,
            state,
            tailscale,
            tailnet,
            tailnet_bus,
        }
    }

    /// A second connection to the daemon, who it is as far as the daemon knows: a test that
    /// needs the shell and Settings at once, or an app, in one world.
    pub async fn connect_as(
        &self,
        role: Role,
        app: AppId,
    ) -> (zbus::Connection, Accounts<DbusTransport>) {
        let connection = self.bus.connect().await;
        self.peers.introduce(
            connection.unique_name().expect("unique name").as_str(),
            Caller { app, role },
        );
        let accounts = Accounts::over(DbusTransport::over(connection.clone()));
        (connection, accounts)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
