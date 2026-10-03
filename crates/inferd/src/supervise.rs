//! Drives stoker's pure engine machine (`engine_supervisor::step`) with real time and the three
//! seams (`EngineHost`, `ReadyProbe`, `GpuProbe`). One task owns the `Supervisor`; everything
//! else talks to it through a [`Supervised`] handle (commands in, snapshots out), so the rest of
//! inferd names none of the three seams' types and a test swaps them for stoker's fakes.
//!
//! Time is `tokio::time::Instant` from the driver's start, so a test with paused time is
//! deterministic.

use engine_supervisor::{
    EngineHost, EngineId, EngineSpec, EngineState, ExitCode, GpuMemory, GpuProbe, MonoMs,
    ReadyProbe, Supervisor, SupervisorConfig, SupervisorIn, SupervisorOut, step,
};
use model_catalog::MiB;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

/// The engine could not be brought up (the failure and its numbers are in the snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed;

/// What every engine is doing, as of the last step.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// The state of each engine.
    pub states: BTreeMap<EngineId, EngineState>,
    /// The driver's clock when the snapshot was taken.
    pub now: Option<MonoMs>,
}

#[derive(Debug)]
enum Command {
    Want(EngineId, oneshot::Sender<()>),
    Used(EngineId),
}

/// The seams the driver is built over.
#[derive(Debug)]
pub struct Ports<H, P, G> {
    /// Starts and stops the engine processes.
    pub host: H,
    /// Asks whether an engine answers.
    pub probe: P,
    /// Reads the GPU's memory.
    pub gpu: G,
}

/// A handle on the driver task. Cloning shares it; the task ends when the last clone is dropped.
#[derive(Debug, Clone)]
pub struct Supervised {
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<Snapshot>,
    probe_every: Duration,
}

impl Supervised {
    /// A supervisor of no engines: every `want` fails. For daemons with nothing to run.
    pub fn idle() -> Self {
        let (commands, _) = mpsc::unbounded_channel();
        let (_, state) = watch::channel(Snapshot::default());
        Self {
            commands,
            state,
            probe_every: SupervisorConfig::default().probe_every,
        }
    }

    /// Starts the driver over `specs` and `ports`. Must be called inside a tokio runtime.
    pub fn start<H, P, G>(
        specs: Vec<EngineSpec>,
        config: SupervisorConfig,
        ports: Ports<H, P, G>,
    ) -> Self
    where
        H: EngineHost + 'static,
        P: ReadyProbe + 'static,
        G: GpuProbe + 'static,
    {
        let (commands, inbox) = mpsc::unbounded_channel();
        let ids: Vec<EngineId> = specs.iter().map(|spec| spec.id.clone()).collect();
        // Every engine is known (and stopped) before the driver's first step.
        let (publish, state) = watch::channel(Snapshot {
            states: ids
                .iter()
                .map(|id| (id.clone(), EngineState::Stopped))
                .collect(),
            now: None,
        });
        let nothing = GpuMemory {
            total: MiB(0),
            used_by_others: MiB(0),
        };
        let driver = Driver {
            sup: Supervisor::new(specs, nothing),
            ids,
            config,
            ports: Arc::new(ports),
            tasks: JoinSet::new(),
            wakes: BTreeSet::new(),
            origin: Instant::now(),
            publish,
        };
        tokio::spawn(driver.run(inbox));
        Self {
            commands,
            state,
            probe_every: config.probe_every,
        }
    }

    /// Asks for the engine and waits until it is ready (`Ok`) or has failed for good (`Failed`).
    /// Dropping the future abandons the wait, not the engine.
    pub async fn want(&self, id: &EngineId) -> Result<(), Failed> {
        let (ack, done) = oneshot::channel();
        self.commands
            .send(Command::Want(id.clone(), ack))
            .map_err(|_| Failed)?;
        done.await.map_err(|_| Failed)?;
        let mut state = self.state.clone();
        loop {
            match state.borrow_and_update().states.get(id) {
                Some(EngineState::Ready { .. }) => return Ok(()),
                Some(EngineState::Failed(_)) | None => return Err(Failed),
                Some(_) => {}
            }
            state.changed().await.map_err(|_| Failed)?;
        }
    }

    /// Asks for the engine without waiting (`Prepare`).
    pub fn warm(&self, id: &EngineId) {
        let (ack, _) = oneshot::channel();
        let _ = self.commands.send(Command::Want(id.clone(), ack));
    }

    /// A turn is using the engine: it is not idle and never a victim of eviction.
    pub fn used(&self, id: &EngineId) {
        let _ = self.commands.send(Command::Used(id.clone()));
    }

    /// What every engine is doing now.
    pub fn snapshot(&self) -> Snapshot {
        self.state.borrow().clone()
    }

    /// How recently an engine must have been used to count as in a turn.
    pub fn probe_every(&self) -> Duration {
        self.probe_every
    }

    /// Resolves when any engine's state changes (for `EnginesChanged`).
    pub async fn changed(&self) -> Result<(), Failed> {
        self.state.clone().changed().await.map_err(|_| Failed)
    }
}

struct Driver<H, P, G> {
    sup: Supervisor,
    ids: Vec<EngineId>,
    config: SupervisorConfig,
    ports: Arc<Ports<H, P, G>>,
    tasks: JoinSet<SupervisorIn>,
    wakes: BTreeSet<MonoMs>,
    origin: Instant,
    publish: watch::Sender<Snapshot>,
}

impl<H, P, G> Driver<H, P, G>
where
    H: EngineHost + 'static,
    P: ReadyProbe + 'static,
    G: GpuProbe + 'static,
{
    fn now(&self) -> MonoMs {
        MonoMs(u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Command>) {
        loop {
            let next = self
                .wakes
                .first()
                .map(|at| self.origin + Duration::from_millis(at.0));
            tokio::select! {
                command = inbox.recv() => match command {
                    None => break,
                    Some(command) => self.command(command).await,
                },
                Some(joined) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    if let Ok(input) = joined {
                        self.pump(vec![input]).await;
                    }
                }
                () = async { match next { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } }, if next.is_some() => {
                    let now = self.now();
                    self.wakes.retain(|at| *at > now);
                    self.pump(Vec::new()).await;
                }
            }
        }
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Used(id) => self.pump(vec![SupervisorIn::Used(id)]).await,
            Command::Want(id, ack) => {
                let memory = self.ports.gpu.memory().await.unwrap_or(GpuMemory {
                    total: MiB(0),
                    used_by_others: MiB(0),
                });
                self.pump(vec![SupervisorIn::Gpu(memory), SupervisorIn::Want(id)])
                    .await;
                let _ = ack.send(());
            }
        }
    }

    /// Ticks, feeds the inputs and everything they cause, then publishes the states.
    async fn pump(&mut self, first: Vec<SupervisorIn>) {
        let mut queue: VecDeque<SupervisorIn> = VecDeque::from([SupervisorIn::Tick(self.now())]);
        queue.extend(first);
        while let Some(input) = queue.pop_front() {
            let (next, effects) = step(self.sup.clone(), input, &self.config);
            self.sup = next;
            for effect in effects {
                if let Some(follow) = self.apply(effect).await {
                    queue.push_back(follow);
                }
            }
        }
        let states = self
            .ids
            .iter()
            .filter_map(|id| self.sup.state(id).map(|state| (id.clone(), *state)))
            .collect();
        self.publish.send_replace(Snapshot {
            states,
            now: Some(self.now()),
        });
    }

    /// Carries out one effect; an input it causes at once is returned.
    async fn apply(&mut self, effect: SupervisorOut) -> Option<SupervisorIn> {
        let ports = Arc::clone(&self.ports);
        match effect {
            SupervisorOut::Spawn(id, unit) => match ports.host.spawn(&id, &unit).await {
                Ok(()) => {
                    self.tasks.spawn(async move {
                        let code = ports.host.exited(&id).await;
                        SupervisorIn::Exited { id, code }
                    });
                    None
                }
                Err(_) => Some(SupervisorIn::Exited {
                    id,
                    code: ExitCode(-1),
                }),
            },
            SupervisorOut::Stop(id) => {
                let _ = ports.host.stop(&id).await;
                None
            }
            SupervisorOut::Probe(id) => {
                self.tasks.spawn(async move {
                    let probe = ports.probe.probe(&id).await;
                    SupervisorIn::Probed { id, probe }
                });
                None
            }
            SupervisorOut::WakeAt(at) => {
                self.wakes.insert(at);
                None
            }
            SupervisorOut::Changed(..) => None,
        }
    }
}

#[cfg(test)]
mod tests;
