//! The real seams of the engine supervisor: child processes, a health probe over the engine's
//! socket, and `nvidia-smi`. The tests of the machine use stoker's fakes; these are exercised
//! with harmless programs (`sleep`, a shell script) and never with an engine.
//!
//! `ProcessHost` is the unconfined host (models section 3.9: for desktops without systemd, and
//! marked so). The systemd transient-unit host, which gives an engine its sandbox
//! (`PrivateNetwork`, a read-only home, the GPU device and a memory cap), is not built; the unit
//! spec already carries the sandbox for it.

use engine_supervisor::{
    EngineHost, EngineId, ExitCode, GpuError, GpuMemory, GpuProbe, HostError, Probe, ReadyProbe,
    UnitSpec,
};
use model_catalog::MiB;
use model_http::{
    AuthHeader, BodySink, ChunkFlow, Exchange, Framing, HttpClient, HttpEndpoint, HttpTarget,
    ResponseHead, RouteRoot, Timeouts, Transport, UrlPath, Verb, WaitMs,
};
use speech_host_client::{HostSocket, SpeechHostClient};
use speech_provider::SpeechToText;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

/// What the monitor of one child process publishes: nothing yet, or its exit code.
type Exit = watch::Receiver<Option<ExitCode>>;

struct Running {
    stop: Option<oneshot::Sender<()>>,
    exit: Exit,
}

/// Engines as child processes of the daemon.
#[derive(Default)]
pub struct ProcessHost {
    running: Arc<Mutex<BTreeMap<EngineId, Running>>>,
}

impl std::fmt::Debug for ProcessHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessHost")
    }
}

impl ProcessHost {
    /// A host with nothing running.
    pub fn new() -> Self {
        Self::default()
    }

    fn table(&self) -> std::sync::MutexGuard<'_, BTreeMap<EngineId, Running>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EngineHost for ProcessHost {
    async fn spawn(&self, id: &EngineId, unit: &UnitSpec) -> Result<(), HostError> {
        let mut command = Command::new(&unit.program.0);
        command
            .args(unit.args.iter().map(|arg| &arg.0))
            .envs(unit.env.iter().map(|pair| (&pair.name, &pair.value)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| HostError::Refused)?;
        let (stop, stopped) = oneshot::channel::<()>();
        let (publish, exit) = watch::channel(None);
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status.ok(),
                _ = stopped => {
                    let _ = child.kill().await;
                    child.wait().await.ok()
                }
            };
            let code = status.and_then(|status| status.code()).unwrap_or(-1);
            let _ = publish.send(Some(ExitCode(code)));
        });
        self.table().insert(
            id.clone(),
            Running {
                stop: Some(stop),
                exit,
            },
        );
        Ok(())
    }

    async fn stop(&self, id: &EngineId) -> Result<(), HostError> {
        let stop = self
            .table()
            .get_mut(id)
            .and_then(|running| running.stop.take());
        match stop {
            Some(stop) => {
                let _ = stop.send(());
                Ok(())
            }
            None => Err(HostError::NotRunning),
        }
    }

    async fn exited(&self, id: &EngineId) -> ExitCode {
        let exit = self.table().get(id).map(|running| running.exit.clone());
        let Some(mut exit) = exit else {
            return ExitCode(-1);
        };
        loop {
            if let Some(code) = *exit.borrow_and_update() {
                return code;
            }
            if exit.changed().await.is_err() {
                return ExitCode(-1);
            }
        }
    }
}

/// `GET /health` over each engine's socket (llama-server and vLLM both answer it at the server
/// root: 200 when ready, 503 while the model loads). A speech host does not speak HTTP: its
/// engines are probed with the host protocol's `Hello` ([`HealthProbe::speech_hosts`]), which it
/// answers once its model is loaded and it is accepting connections.
#[derive(Debug, Clone, Default)]
pub struct HealthProbe {
    sockets: BTreeMap<EngineId, PathBuf>,
    speech: BTreeSet<EngineId>,
}

impl HealthProbe {
    /// A probe for these engines' sockets.
    pub fn new(sockets: impl IntoIterator<Item = (EngineId, PathBuf)>) -> Self {
        Self {
            sockets: sockets.into_iter().collect(),
            speech: BTreeSet::new(),
        }
    }

    /// The same probe, asking these engines (speech hosts) `Hello` instead of `GET /health`.
    pub fn speech_hosts(self, engines: impl IntoIterator<Item = EngineId>) -> Self {
        Self {
            speech: engines.into_iter().collect(),
            ..self
        }
    }
}

/// The status of the one response a health probe reads.
#[derive(Debug, Default)]
struct StatusOnly(Option<u16>);

impl BodySink for StatusOnly {
    fn head(&mut self, head: &ResponseHead) -> ChunkFlow {
        self.0 = Some(head.status.0);
        ChunkFlow::Stop
    }

    fn chunk(&mut self, _: &[u8]) -> ChunkFlow {
        ChunkFlow::Stop
    }
}

/// How long a probe waits at each stage: an engine that does not answer in a second is not
/// ready yet.
const PROBE_TIMEOUT: WaitMs = WaitMs(1000);

/// An endpoint on a Unix socket with no auth and no `/v1` base.
pub fn unix_endpoint(socket: PathBuf, base: &str, timeouts: Timeouts) -> HttpEndpoint {
    HttpEndpoint {
        target: HttpTarget::Unix(socket),
        proxy: model_http::Proxy::Direct,
        base: UrlPath(base.to_owned()),
        auth: AuthHeader::None,
        headers: Vec::new(),
        timeouts,
    }
}

/// An endpoint on `127.0.0.1` at `port` over plain HTTP, with no auth: a runtime the person runs.
pub fn loopback_endpoint(port: model_http::Port, base: &str, timeouts: Timeouts) -> HttpEndpoint {
    HttpEndpoint {
        target: HttpTarget::Tcp {
            host: model_http::HostName("127.0.0.1".to_owned()),
            port,
        },
        proxy: model_http::Proxy::Direct,
        base: UrlPath(base.to_owned()),
        auth: AuthHeader::None,
        headers: Vec::new(),
        timeouts,
    }
}

impl ReadyProbe for HealthProbe {
    async fn probe(&self, id: &EngineId) -> Probe {
        let Some(socket) = self.sockets.get(id) else {
            return Probe::Down;
        };
        if self.speech.contains(id) {
            return hello(socket).await;
        }
        let client = HttpClient::new(unix_endpoint(
            socket.clone(),
            "",
            Timeouts {
                connect: PROBE_TIMEOUT,
                first_byte: PROBE_TIMEOUT,
                idle: PROBE_TIMEOUT,
            },
        ));
        let exchange = Exchange {
            verb: Verb::Get,
            root: RouteRoot::Server,
            path: UrlPath("/health".into()),
            body: None,
            framing: Framing::Whole,
        };
        let mut sink = StatusOnly::default();
        let _ = client.exchange(&exchange, &mut sink).await;
        match sink.0 {
            Some(200) => Probe::Ready,
            Some(503) => Probe::Loading,
            _ => Probe::Down,
        }
    }
}

/// A speech host is ready when it answers `Hello` (checked, within a second); not yet listening,
/// or not answering, is down.
async fn hello(socket: &std::path::Path) -> Probe {
    let client = SpeechHostClient::new(HostSocket(socket.to_path_buf()));
    match tokio::time::timeout(
        Duration::from_millis(u64::from(PROBE_TIMEOUT.0)),
        client.describe(),
    )
    .await
    {
        Ok(Ok(_)) => Probe::Ready,
        _ => Probe::Down,
    }
}

/// The GPU as `nvidia-smi` reports it: the total memory. What other programs use is not read
/// (`used_by_others` is zero): `nvidia-smi` counts our own engines in it, and subtracting them
/// needs per-process accounting that is not built. A GPU another program fills therefore shows
/// as an engine that does not start, not as a refusal with numbers.
#[derive(Debug, Clone)]
pub struct NvidiaSmi {
    program: PathBuf,
}

impl NvidiaSmi {
    /// Runs this `nvidia-smi`.
    pub fn new(program: PathBuf) -> Self {
        Self { program }
    }
}

/// The total of the first GPU from `--query-gpu=memory.total --format=csv,noheader,nounits`.
pub fn parse_total(text: &str) -> Result<MiB, GpuError> {
    text.lines()
        .next()
        .map(str::trim)
        .and_then(|line| line.parse::<u32>().ok())
        .map(MiB)
        .ok_or(GpuError::Unreadable)
}

impl GpuProbe for NvidiaSmi {
    async fn memory(&self) -> Result<GpuMemory, GpuError> {
        let output = Command::new(&self.program)
            .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| GpuError::Unavailable)?;
        if !output.status.success() {
            return Err(GpuError::Unavailable);
        }
        Ok(GpuMemory {
            total: parse_total(&String::from_utf8_lossy(&output.stdout))?,
            used_by_others: MiB(0),
        })
    }
}

#[cfg(test)]
mod tests;
