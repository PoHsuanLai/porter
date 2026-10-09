//! The real seams of the engine supervisor: child processes, a health probe over the engine's
//! socket, and `nvidia-smi`. The tests of the machine use stoker's fakes; these are exercised
//! with harmless programs (`sleep`, a shell script) and never with an engine.
//!
//! `ProcessHost` is the unconfined host (models section 3.9: for desktops without systemd, and
//! marked so). The systemd transient-unit host, which gives an engine its sandbox
//! (`PrivateNetwork`, a read-only home, the GPU device and a memory cap), is not built; the unit
//! spec already carries the sandbox for it.

use crate::startup::{Cause, Diagnostics, TailBuf, clear_socket};
use engine_supervisor::{
    EngineHost, EngineId, ExitCode, GpuError, GpuMemory, GpuProbe, HostError, Probe, ReadyProbe,
    UnitSpec,
};
use model_catalog::MiB;
use model_http::{
    BodySink, ChunkFlow, Exchange, Framing, HttpClient, ResponseHead, RouteRoot, Timeouts,
    Transport, UrlPath, Verb, WaitMs,
};
use rustix::process::Pid;
use speech_host_client::{HostSocket, SpeechHostClient};
use speech_provider::SpeechToText;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

/// What the monitor of one child process publishes: nothing yet, or its exit code.
type Exit = watch::Receiver<Option<ExitCode>>;

struct Running {
    stop: Option<oneshot::Sender<()>>,
    exit: Exit,
    /// The engine's process group while any of it may be running; the monitor clears it once the
    /// group is gone, so a group id the system has reused is never signalled.
    group: Arc<Mutex<Option<Pid>>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let group = self.group.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(group) = *group {
            group::sweep(group);
        }
    }
}

/// How long the end of a process waits for its standard error to end too (a grandchild that holds
/// the pipe open must not hold the exit back).
const STDERR_GRACE: Duration = Duration::from_secs(1);

/// How long an engine that was sent `SIGTERM` has to go (the leader, then the rest of its group)
/// before the group is killed. Under the unit's `TimeoutStopSec=20`.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// Engines as child processes of the daemon. Told each engine's socket
/// ([`ProcessHost::with_sockets`]) it looks at the path before it spawns: a path too long to bind
/// or something that is not a socket is refused, a socket an earlier run left is removed. The
/// standard error of every process is read (and passed on to ours); the last lines of it, and why
/// a spawn was refused, are in the [`Diagnostics`] the driver reads.
///
/// Each engine leads a process group of its own and dies with inferd (see [`group`]): ending it
/// signals the whole group (`SIGTERM`, [`STOP_GRACE`], `SIGKILL`), dropping the host kills every
/// group still running.
pub struct ProcessHost {
    running: Arc<Mutex<BTreeMap<EngineId, Running>>>,
    closed: Arc<AtomicBool>,
    sockets: BTreeMap<EngineId, PathBuf>,
    diagnostics: Diagnostics,
    grace: Duration,
}

type Table = Arc<Mutex<BTreeMap<EngineId, Running>>>;

/// A handle on a [`ProcessHost`] that outlives the move of the host into the supervisor: it ends
/// every engine when the daemon is told to stop ([`crate::shutdown`]). Once [`HostCloser::end_all`]
/// has begun the host starts nothing more.
#[derive(Clone)]
pub struct HostCloser {
    running: Table,
    closed: Arc<AtomicBool>,
}

impl std::fmt::Debug for HostCloser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostCloser")
    }
}

impl HostCloser {
    /// Refuses new engines, then stops every engine the way a stop does (the group is sent
    /// `SIGTERM`, given the host's grace, then killed) and returns when each has gone. At most two
    /// graces plus the time the engines take to be reaped.
    pub async fn end_all(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let exits: Vec<Exit> = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values_mut()
            .map(|running| {
                if let Some(stop) = running.stop.take() {
                    let _ = stop.send(());
                }
                running.exit.clone()
            })
            .collect();
        for mut exit in exits {
            while exit.borrow_and_update().is_none() {
                if exit.changed().await.is_err() {
                    break;
                }
            }
        }
    }

    /// `SIGKILL` to every group still running, at once, and nothing waited for.
    pub fn kill_all(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for running in self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            let group = running.group.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(group) = *group {
                group::sweep(group);
            }
        }
    }
}

impl Default for ProcessHost {
    fn default() -> Self {
        Self {
            running: Arc::default(),
            closed: Arc::default(),
            sockets: BTreeMap::new(),
            diagnostics: Diagnostics::default(),
            grace: STOP_GRACE,
        }
    }
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

    /// The same, checking each of these engines' socket before it spawns.
    pub fn with_sockets(self, sockets: impl IntoIterator<Item = (EngineId, PathBuf)>) -> Self {
        Self {
            sockets: sockets.into_iter().collect(),
            ..self
        }
    }

    /// The same, giving a stopped engine `grace` to go before its group is killed.
    pub fn with_grace(self, grace: Duration) -> Self {
        Self { grace, ..self }
    }

    /// What this host learns about starts: give it to [`crate::supervise::Supervised::start_with`].
    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics.clone()
    }

    /// The handle that ends every engine of this host at shutdown.
    pub fn closer(&self) -> HostCloser {
        HostCloser {
            running: Arc::clone(&self.running),
            closed: Arc::clone(&self.closed),
        }
    }

    fn table(&self) -> std::sync::MutexGuard<'_, BTreeMap<EngineId, Running>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Reads a process's standard error to its end: into the bounded tail, and on to ours.
async fn drain(mut stderr: tokio::process::ChildStderr, live: Arc<Mutex<TailBuf>>) {
    let mut chunk = [0u8; 4096];
    loop {
        match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                live.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(&chunk[..n]);
                // The engine's own words still reach inferd's standard error (and the journal).
                let _ = std::io::Write::write_all(&mut std::io::stderr(), &chunk[..n]);
            }
        }
    }
    live.lock().unwrap_or_else(PoisonError::into_inner).finish();
}

impl EngineHost for ProcessHost {
    async fn spawn(&self, id: &EngineId, unit: &UnitSpec) -> Result<(), HostError> {
        if self.closed.load(Ordering::SeqCst) {
            // The daemon is stopping: nothing new starts.
            return Err(HostError::Refused);
        }
        let live = self.diagnostics.begin(id);
        if let Some(socket) = self.sockets.get(id)
            && let Err(cause) = clear_socket(socket)
        {
            self.diagnostics.refuse(id, cause);
            return Err(HostError::Refused);
        }
        let mut command = Command::new(&unit.program.0);
        command
            .args(unit.args.iter().map(|arg| &arg.0))
            .envs(unit.env.iter().map(|pair| (&pair.name, &pair.value)))
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        group::die_with_parent(&mut command);
        let mut child = command.spawn().map_err(|e| {
            self.diagnostics.refuse(
                id,
                Cause::CannotSpawn {
                    program: unit.program.0.clone(),
                    kind: e.kind(),
                },
            );
            HostError::Refused
        })?;
        let reader = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(drain(stderr, live)));
        let (stop, stopped) = oneshot::channel::<()>();
        let (publish, exit) = watch::channel(None);
        let leads = group::group_of(&child);
        let group = Arc::new(Mutex::new(leads));
        let grace = self.grace;
        let held = Arc::clone(&group);
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => {
                    // It ended by itself: what it left running in its group goes with it.
                    if let Some(group) = leads {
                        group::sweep(group);
                    }
                    status.ok()
                }
                _ = stopped => {
                    // Told to stop: the group is asked to go, the leader is waited for, and
                    // whatever of the group is still there when the grace is over is killed.
                    if let Some(group) = leads {
                        group::terminate(group);
                    }
                    let status = match tokio::time::timeout(grace, child.wait()).await {
                        Ok(status) => status.ok(),
                        Err(_) => {
                            if let Some(group) = leads {
                                group::sweep(group);
                            }
                            let _ = child.kill().await;
                            child.wait().await.ok()
                        }
                    };
                    if let Some(group) = leads {
                        group::end(group, grace).await;
                    }
                    status
                }
            };
            *held.lock().unwrap_or_else(PoisonError::into_inner) = None;
            // Its last words are in the tail before anyone is told it is gone.
            if let Some(reader) = reader {
                let _ = tokio::time::timeout(STDERR_GRACE, reader).await;
            }
            let code = status.and_then(|status| status.code()).unwrap_or(-1);
            let _ = publish.send(Some(ExitCode(code)));
        });
        let mut table = self.table();
        // A shutdown that began while this child was starting has already gone through the table:
        // this one is stopped here, under the same lock.
        let stop = if self.closed.load(Ordering::SeqCst) {
            let _ = stop.send(());
            None
        } else {
            Some(stop)
        };
        table.insert(id.clone(), Running { stop, exit, group });
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

/// How long a probe waits at each stage. The supervisor asks again every half second until the
/// engine's start timeout, so a probe that gives up early only wastes a start on a loaded
/// computer, where the answer of an engine that is up can take seconds; a probe that waits ten
/// delays only the next ask of an engine that does not answer at all.
const PROBE_TIMEOUT: WaitMs = WaitMs(10_000);

/// The endpoints of an engine on a Unix socket and of a runtime on loopback moved to
/// `porter_router::local`.
pub use porter_router::local::{loopback_endpoint, unix_endpoint};

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

/// A speech host is ready when it answers `Hello` (checked, within `PROBE_TIMEOUT`); not yet listening,
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

pub mod group;

#[cfg(test)]
mod tests;
