//! Replay engines as an [`EngineHost`]: the engines the configuration names a cassette for are
//! served in this process (a task on the engine's socket), every other engine goes to the host
//! underneath (child processes). The supervisor, the health probe and the runner see no
//! difference: an engine starts, answers `/health`, serves chat on its socket, and unloads.

use super::engine::serve;
use super::replayer::{ReplayError, Replayer};
use engine_supervisor::{EngineHost, EngineId, ExitCode, HostError, UnitSpec};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// One replay engine: where it listens and what it plays (or why it cannot).
#[derive(Debug, Clone)]
pub struct Replaying {
    /// The engine's socket.
    pub socket: PathBuf,
    /// The cassette, read once at startup; a failure refuses the engine's start.
    pub replayer: Result<Arc<Replayer>, ReplayError>,
}

struct Live {
    task: JoinHandle<()>,
    exit: watch::Sender<Option<ExitCode>>,
}

/// The host: replay engines here, the rest to `inner`.
pub struct ReplayHost<H> {
    inner: H,
    engines: BTreeMap<EngineId, Replaying>,
    live: Mutex<BTreeMap<EngineId, Live>>,
}

impl<H> std::fmt::Debug for ReplayHost<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReplayHost")
    }
}

impl<H> ReplayHost<H> {
    /// A host that plays `engines` itself and hands the others to `inner`.
    pub fn new(inner: H, engines: BTreeMap<EngineId, Replaying>) -> Self {
        Self {
            inner,
            engines,
            live: Mutex::new(BTreeMap::new()),
        }
    }

    fn live(&self) -> std::sync::MutexGuard<'_, BTreeMap<EngineId, Live>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(&self, id: &EngineId, replaying: &Replaying) -> Result<(), HostError> {
        let replayer = replaying.replayer.clone().map_err(|_| HostError::Refused)?;
        // A socket left by an earlier run is ours to replace.
        let _ = std::fs::remove_file(&replaying.socket);
        let listener = UnixListener::bind(&replaying.socket).map_err(|_| HostError::Refused)?;
        let (exit, _) = watch::channel(None);
        let task = tokio::spawn(serve(listener, replayer));
        if let Some(old) = self.live().insert(id.clone(), Live { task, exit }) {
            old.task.abort();
        }
        Ok(())
    }
}

impl<H: EngineHost> EngineHost for ReplayHost<H> {
    async fn spawn(&self, id: &EngineId, unit: &UnitSpec) -> Result<(), HostError> {
        match self.engines.get(id) {
            Some(replaying) => self.start(id, replaying),
            None => self.inner.spawn(id, unit).await,
        }
    }

    async fn stop(&self, id: &EngineId) -> Result<(), HostError> {
        if !self.engines.contains_key(id) {
            return self.inner.stop(id).await;
        }
        let live = self.live().remove(id).ok_or(HostError::NotRunning)?;
        live.task.abort();
        let _ = live.exit.send(Some(ExitCode(0)));
        Ok(())
    }

    async fn exited(&self, id: &EngineId) -> ExitCode {
        if !self.engines.contains_key(id) {
            return self.inner.exited(id).await;
        }
        let exit = self.live().get(id).map(|live| live.exit.subscribe());
        let Some(mut exit) = exit else {
            return ExitCode(-1);
        };
        loop {
            if let Some(code) = *exit.borrow_and_update() {
                return code;
            }
            if exit.changed().await.is_err() {
                return ExitCode(0);
            }
        }
    }
}

#[cfg(test)]
mod tests;
