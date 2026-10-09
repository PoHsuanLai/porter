//! Drives stoker's pure engine machine (`engine_supervisor::step`) with real time and the three
//! seams (`EngineHost`, `ReadyProbe`, `GpuProbe`). One task owns the `Supervisor`; everything
//! else talks to it through a [`Supervised`] handle (commands in, snapshots out), so the rest of
//! inferd names none of the three seams' types and a test swaps them for stoker's fakes.
//!
//! Time is `tokio::time::Instant` from the driver's start, so a test with paused time is
//! deterministic.

use crate::startup::{Cause, Diagnostics, Tail};
use engine_supervisor::{
    EngineFailure, EngineHost, EngineId, EngineSpec, EngineState, ExitCode, GpuMemory, GpuProbe,
    MonoMs, ReadyProbe, Supervisor, SupervisorConfig, SupervisorIn, SupervisorOut, step,
};
use model_catalog::MiB;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

/// The engine could not be brought up, and why.
///
/// A process that exits before it is ready fails every waiter at that exit (not after the
/// readiness timeout). The machine goes on to try again in the background: 3 attempts, 2 s then
/// 4 s apart (`SupervisorConfig::max_attempts` and `backoff`), each failing the waiters it has.
/// After the last, requests are failed at once for the longest backoff (30 s by default) and
/// then the engine is tried again from the first attempt; no request restarts it sooner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    /// Why.
    pub cause: Cause,
}

impl Failed {
    /// A failure nothing more is known about.
    pub fn unknown() -> Self {
        Self {
            cause: Cause::Unknown,
        }
    }
}

/// The last failure of an engine to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Why.
    pub cause: Cause,
    /// Which failure this is: later ones have larger numbers (a waiter fails on a number it has
    /// not seen).
    pub seq: u32,
    /// Until when a request is failed at once rather than starting the engine again.
    pub retry_at: MonoMs,
}

/// What every engine is doing, as of the last step.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// The last failure to start of each engine that has had one since it was last ready.
    pub failures: BTreeMap<EngineId, Failure>,
    /// The state of each engine.
    pub states: BTreeMap<EngineId, EngineState>,
    /// The driver's clock when the snapshot was taken.
    pub now: Option<MonoMs>,
    /// The GPU as the driver last read it (each time an engine was asked for); `None` before the
    /// first look. The router reads it to cost a swap.
    pub gpu: Option<GpuMemory>,
}

#[derive(Debug)]
enum Command {
    /// Asks for an engine; answered `Err` at once when it failed so lately that no request may
    /// start it again yet.
    Want(EngineId, oneshot::Sender<Result<(), Cause>>),
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
    headroom: MiB,
}

/// The turns of `porter-turns` tell the supervisor which engine they use (the inherent `used`).
impl porter_turns::local::EngineUse for Supervised {
    fn used(&self, engine: &EngineId) {
        Supervised::used(self, engine);
    }
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
            headroom: SupervisorConfig::default().headroom,
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
        Self::start_with(specs, config, ports, Diagnostics::default())
    }

    /// `start`, with what the host learns about starts (a refused spawn, the last lines an
    /// engine said): the causes of failures carry it, and failures are logged through it.
    pub fn start_with<H, P, G>(
        specs: Vec<EngineSpec>,
        config: SupervisorConfig,
        ports: Ports<H, P, G>,
        diagnostics: Diagnostics,
    ) -> Self
    where
        H: EngineHost + 'static,
        P: ReadyProbe + 'static,
        G: GpuProbe + 'static,
    {
        let (commands, inbox) = mpsc::unbounded_channel();
        let ids: Vec<EngineId> = specs.iter().map(|spec| spec.id.clone()).collect();
        let stopped: BTreeMap<EngineId, EngineState> = ids
            .iter()
            .map(|id| (id.clone(), EngineState::Stopped))
            .collect();
        // Every engine is known (and stopped) before the driver's first step.
        let (publish, state) = watch::channel(Snapshot {
            states: stopped.clone(),
            failures: BTreeMap::new(),
            now: None,
            gpu: None,
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
            gpu: None,
            diagnostics,
            seen: stopped,
            pending: BTreeMap::new(),
            failures: BTreeMap::new(),
            failed: 0,
        };
        tokio::spawn(driver.run(inbox));
        Self {
            commands,
            state,
            probe_every: config.probe_every,
            headroom: config.headroom,
        }
    }

    /// Asks for the engine and waits until it is ready (`Ok`) or fails (`Failed`, with the cause):
    /// at the next failure to start, which is the engine's exit when its process ends, not a
    /// timeout. Dropping the future abandons the wait, not the engine.
    pub async fn want(&self, id: &EngineId) -> Result<(), Failed> {
        // Failures before this call do not fail it; one after it does.
        let seen = self.state.borrow().failures.get(id).map(|f| f.seq);
        let (ack, done) = oneshot::channel();
        self.commands
            .send(Command::Want(id.clone(), ack))
            .map_err(|_| Failed::unknown())?;
        match done.await {
            Err(_) => return Err(Failed::unknown()),
            Ok(Err(cause)) => return Err(Failed { cause }),
            Ok(Ok(())) => {}
        }
        let mut state = self.state.clone();
        loop {
            let verdict = {
                let snap = state.borrow_and_update();
                let failure = snap.failures.get(id);
                match snap.states.get(id) {
                    Some(EngineState::Ready { .. }) => Some(Ok(())),
                    Some(EngineState::Failed(_)) | None => {
                        Some(Err(failure.map_or_else(Failed::unknown, |f| Failed {
                            cause: f.cause.clone(),
                        })))
                    }
                    Some(_) => failure.filter(|f| Some(f.seq) > seen).map(|f| {
                        Err(Failed {
                            cause: f.cause.clone(),
                        })
                    }),
                }
            };
            if let Some(verdict) = verdict {
                return verdict;
            }
            state.changed().await.map_err(|_| Failed::unknown())?;
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

    /// The memory kept free beside an engine (what `budget` is asked with).
    pub fn headroom(&self) -> MiB {
        self.headroom
    }

    /// Resolves when any engine's state changes (for `EnginesChanged`).
    pub async fn changed(&self) -> Result<(), Failed> {
        self.state
            .clone()
            .changed()
            .await
            .map_err(|_| Failed::unknown())
    }
}

struct Driver<H, P, G> {
    sup: Supervisor,
    ids: Vec<EngineId>,
    config: SupervisorConfig,
    ports: Arc<Ports<H, P, G>>,
    tasks: JoinSet<Joined>,
    wakes: BTreeSet<MonoMs>,
    origin: Instant,
    publish: watch::Sender<Snapshot>,
    gpu: Option<GpuMemory>,
    diagnostics: Diagnostics,
    /// Each engine's state as of the last change reported, to tell what it changed from.
    seen: BTreeMap<EngineId, EngineState>,
    /// Why an engine's start is ending, known before the machine says it has: a process's exit,
    /// a spawn the host refused.
    pending: BTreeMap<EngineId, Cause>,
    failures: BTreeMap<EngineId, Failure>,
    /// How many failures there have been (their numbers).
    failed: u32,
}

/// What a task the driver waits on brings back: an input for the machine, and for a process's
/// exit what it said last.
struct Joined {
    input: SupervisorIn,
    tail: Option<Tail>,
}

impl From<SupervisorIn> for Joined {
    fn from(input: SupervisorIn) -> Self {
        Self { input, tail: None }
    }
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
                    if let Ok(joined) = joined {
                        self.joined(joined).await;
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
                // No request starts an engine again while it is paused after failing.
                if let Some(cause) = self.paused(&id) {
                    let _ = ack.send(Err(cause));
                    return;
                }
                let memory = self.ports.gpu.memory().await.unwrap_or(GpuMemory {
                    total: MiB(0),
                    used_by_others: MiB(0),
                });
                self.gpu = Some(memory);
                self.pump(vec![SupervisorIn::Gpu(memory), SupervisorIn::Want(id)])
                    .await;
                let _ = ack.send(Ok(()));
            }
        }
    }

    /// The cause of the failure an engine is paused after: it has failed for good (the machine
    /// tried again and gave up) and the pause has not run out.
    fn paused(&self, id: &EngineId) -> Option<Cause> {
        let failure = self.failures.get(id)?;
        match self.sup.state(id) {
            Some(EngineState::Failed(_))
                if failure.cause.pauses() && failure.retry_at > self.now() =>
            {
                Some(failure.cause.clone())
            }
            _ => None,
        }
    }

    /// A task finished: a process's exit while it was starting or running is the cause of the
    /// failure the machine is about to see.
    async fn joined(&mut self, joined: Joined) {
        if let SupervisorIn::Exited { id, code } = &joined.input
            && matches!(
                self.seen.get(id),
                Some(EngineState::Starting { .. } | EngineState::Ready { .. })
            )
        {
            self.pending.insert(
                id.clone(),
                Cause::Exited {
                    code: *code,
                    tail: joined.tail.unwrap_or_default(),
                },
            );
        }
        self.pump(vec![joined.input]).await;
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
            failures: self.failures.clone(),
            now: Some(self.now()),
            gpu: self.gpu,
        });
    }

    /// The machine changed an engine's state: a start that ends is a failure with a cause, an
    /// engine that is ready has none.
    fn changed(&mut self, id: EngineId, state: EngineState) {
        let before = self.seen.insert(id.clone(), state);
        let started = matches!(
            before,
            Some(EngineState::Starting { .. } | EngineState::Ready { .. })
        );
        let cause = match state {
            EngineState::Ready { .. } => {
                self.failures.remove(&id);
                self.pending.remove(&id);
                return;
            }
            EngineState::Backoff { .. }
            | EngineState::Failed(EngineFailure::Exited { .. } | EngineFailure::NeverReady)
                if started =>
            {
                self.pending
                    .remove(&id)
                    .unwrap_or_else(|| Cause::NeverReady {
                        tail: self.diagnostics.tail(&id),
                    })
            }
            EngineState::Failed(EngineFailure::NoRoom { need, free }) => {
                Cause::NoRoom { need, free }
            }
            EngineState::Failed(EngineFailure::BadProfile) => Cause::BadProfile,
            _ => return,
        };
        self.diagnostics.log().failed(&id, &cause);
        self.failed = self.failed.saturating_add(1);
        let pause = self.config.backoff.1;
        let retry_at = MonoMs(
            self.now()
                .0
                .saturating_add(u64::try_from(pause.as_millis()).unwrap_or(u64::MAX)),
        );
        // A tick when the pause ends, so the snapshot's clock moves past it and the engine reads
        // as loadable again.
        self.wakes.insert(retry_at);
        self.failures.insert(
            id,
            Failure {
                cause,
                seq: self.failed,
                retry_at,
            },
        );
    }

    /// Carries out one effect; an input it causes at once is returned.
    async fn apply(&mut self, effect: SupervisorOut) -> Option<SupervisorIn> {
        let ports = Arc::clone(&self.ports);
        match effect {
            SupervisorOut::Spawn(id, unit) => match ports.host.spawn(&id, &unit).await {
                Ok(()) => {
                    let diagnostics = self.diagnostics.clone();
                    self.tasks.spawn(async move {
                        let code = ports.host.exited(&id).await;
                        let tail = Some(diagnostics.tail(&id));
                        Joined {
                            input: SupervisorIn::Exited { id, code },
                            tail,
                        }
                    });
                    None
                }
                Err(_) => {
                    let cause = self.diagnostics.refusal(&id).unwrap_or(Cause::Refused);
                    self.pending.insert(id.clone(), cause);
                    Some(SupervisorIn::Exited {
                        id,
                        code: ExitCode(-1),
                    })
                }
            },
            SupervisorOut::Stop(id) => {
                let _ = ports.host.stop(&id).await;
                None
            }
            SupervisorOut::Probe(id) => {
                self.tasks.spawn(async move {
                    let probe = ports.probe.probe(&id).await;
                    SupervisorIn::Probed { id, probe }.into()
                });
                None
            }
            SupervisorOut::WakeAt(at) => {
                self.wakes.insert(at);
                None
            }
            SupervisorOut::Changed(id, state) => {
                self.changed(id, state);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
