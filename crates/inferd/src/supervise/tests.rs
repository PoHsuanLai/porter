//! The driver over stoker's seams, with the clock paused: time moves only when every task waits,
//! so backoffs and the idle timer cost nothing.

use super::*;
use crate::startup::{Level, Log};
use crate::testkit::{Recorder, Scratch, models};
use engine_supervisor::{EngineFailure, FakeEngineHost, FakeGpu, FakeReadyProbe, Probe};
use std::sync::Mutex;

fn gpu(total: u32) -> FakeGpu {
    FakeGpu(GpuMemory {
        total: MiB(total),
        used_by_others: MiB(0),
    })
}

fn specs(scratch: &Scratch) -> Vec<EngineSpec> {
    models(scratch).into_iter().map(|m| m.spec).collect()
}

fn state(supervised: &Supervised, id: &EngineId) -> EngineState {
    *supervised.snapshot().states.get(id).expect("known engine")
}

#[tokio::test(start_paused = true)]
async fn the_first_want_starts_the_engine_and_the_second_reuses_it() {
    let scratch = Scratch::new("sup-reuse");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let host = Recorder::default();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert!(matches!(state(&supervised, &id), EngineState::Stopped));
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert!(matches!(state(&supervised, &id), EngineState::Ready { .. }));
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert_eq!(*host.spawned.lock().expect("lock"), vec![id]);
}

#[tokio::test(start_paused = true)]
async fn a_listener_wakes_once_for_a_change_and_not_again_until_the_next() {
    let scratch = Scratch::new("sup-changes");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    let quiet = std::time::Duration::from_secs(60);
    let mut changes = supervised.changes();
    assert_eq!(supervised.want(&id).await, Ok(()));
    // The start is told...
    changes.next().await.expect("a change");
    // ...and once it has been, nothing more is until something changes again: a listener that
    // woke at once every time made the bus loop send EnginesChanged without end.
    let woke = tokio::time::timeout(quiet, changes.next()).await;
    assert!(woke.is_err(), "woke with nothing changed");
    // A listener made after the start has nothing to hear either.
    let mut late = supervised.changes();
    let woke = tokio::time::timeout(quiet, late.next()).await;
    assert!(woke.is_err(), "a new listener woke for an old change");
    supervised.release(&id);
    changes.next().await.expect("the stop is told");
}

#[tokio::test(start_paused = true)]
async fn an_engine_that_answers_loading_is_waited_for() {
    let scratch = Scratch::new("sup-loading");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    // The probe says Loading twice, then Ready.
    #[derive(Debug, Clone)]
    struct Slow(Arc<Mutex<u32>>);
    impl ReadyProbe for Slow {
        async fn probe(&self, _: &EngineId) -> Probe {
            let mut asked = self.0.lock().expect("lock");
            *asked += 1;
            if *asked > 2 {
                Probe::Ready
            } else {
                Probe::Loading
            }
        }
    }
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: Slow(Arc::default()),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert!(matches!(state(&supervised, &id), EngineState::Ready { .. }));
}

#[tokio::test(start_paused = true)]
async fn an_idle_engine_unloads_and_a_later_want_starts_it_again() {
    let scratch = Scratch::new("sup-idle");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let host = Recorder::default();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(supervised.want(&id).await, Ok(()));
    // The idle timeout is 600 s; nothing uses the engine.
    tokio::time::sleep(Duration::from_secs(601)).await;
    assert_eq!(*host.stopped.lock().expect("lock"), vec![id.clone()]);
    assert!(matches!(state(&supervised, &id), EngineState::Stopped));
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert_eq!(host.spawned.lock().expect("lock").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_released_engine_stops_and_stays_stopped_until_wanted_again() {
    let scratch = Scratch::new("sup-release");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let host = Recorder::default();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(supervised.want(&id).await, Ok(()));
    supervised.release(&id);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*host.stopped.lock().expect("lock"), vec![id.clone()]);
    assert!(matches!(state(&supervised, &id), EngineState::Stopped));
    // Releasing a stopped engine does nothing.
    supervised.release(&id);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(host.stopped.lock().expect("lock").len(), 1);
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert_eq!(host.spawned.lock().expect("lock").len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_wait_for_an_engine_the_person_stops_ends_with_that_instead_of_hanging() {
    let scratch = Scratch::new("sup-release-wait");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    // An engine that is always still loading, as the real probe takes its time.
    #[derive(Debug, Clone)]
    struct Loading;
    impl ReadyProbe for Loading {
        async fn probe(&self, _: &EngineId) -> Probe {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Probe::Loading
        }
    }
    let host = Recorder::default();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: Loading,
            gpu: gpu(16_000),
        },
    );
    let waiting = tokio::spawn({
        let (supervised, id) = (supervised.clone(), id.clone());
        async move { supervised.want(&id).await }
    });
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(matches!(
        state(&supervised, &id),
        EngineState::Starting { .. }
    ));
    supervised.release(&id);
    let ended = waiting.await.expect("the wait ends");
    assert_eq!(
        ended,
        Err(Failed {
            cause: Cause::Released
        })
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(matches!(state(&supervised, &id), EngineState::Stopped));
}

/// A host that records the program of every unit it is asked to start.
#[derive(Debug, Clone, Default)]
struct Programmed {
    started: Arc<Mutex<Vec<std::path::PathBuf>>>,
    gone: Arc<tokio::sync::Notify>,
}

impl EngineHost for Programmed {
    async fn spawn(
        &self,
        _: &EngineId,
        unit: &engine_supervisor::UnitSpec,
    ) -> Result<(), engine_supervisor::HostError> {
        self.started
            .lock()
            .expect("lock")
            .push(unit.program.0.clone());
        Ok(())
    }

    async fn stop(&self, _: &EngineId) -> Result<(), engine_supervisor::HostError> {
        self.gone.notify_waiters();
        Ok(())
    }

    async fn exited(&self, _: &EngineId) -> ExitCode {
        self.gone.notified().await;
        ExitCode(0)
    }
}

#[tokio::test(start_paused = true)]
async fn the_next_start_of_an_engine_uses_the_program_named_by_then() {
    use std::path::PathBuf;
    let scratch = Scratch::new("sup-programs");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let made = specs[0].unit.program.0.clone();
    // Whichever kind the model's engine is, its program is the one the units were made with.
    let config = |path: Option<&PathBuf>| porter_router::local::EngineConfig {
        vllm_python: path.cloned(),
        llama_server: path.cloned(),
        ..Default::default()
    };
    let programs = Programs::new(&config(Some(&made)), None, None);
    let host = Programmed::default();
    let supervised = Supervised::start_sized(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
        Diagnostics::default(),
        Sizing::default().with_programs(programs.clone()),
    );
    // Nothing named: the engine starts with the program from start.
    assert_eq!(supervised.want(&id).await, Ok(()));
    // The file names another; the running engine is left alone, the next start uses it.
    let other = PathBuf::from("/opt/other/program");
    programs.follow(&config(Some(&other)));
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert_eq!(host.started.lock().expect("lock").len(), 1);
    supervised.release(&id);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert_eq!(*host.started.lock().expect("lock"), [made.clone(), other]);
    // Cleared again, and nothing found: the program from start.
    programs.follow(&config(None));
    supervised.release(&id);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert_eq!(host.started.lock().expect("lock")[2], made);
}

#[tokio::test(start_paused = true)]
async fn an_engine_in_use_is_not_unloaded() {
    let scratch = Scratch::new("sup-used");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let host = Recorder::default();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(supervised.want(&id).await, Ok(()));
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_secs(300)).await;
        supervised.used(&id);
    }
    assert!(host.stopped.lock().expect("lock").is_empty());
    assert!(matches!(state(&supervised, &id), EngineState::Ready { .. }));
}

#[tokio::test(start_paused = true)]
async fn an_engine_that_keeps_exiting_fails_for_good_after_its_attempts() {
    let scratch = Scratch::new("sup-crash");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: FakeEngineHost::new(ExitCode(1)),
            probe: FakeReadyProbe(Probe::Down),
            gpu: gpu(16_000),
        },
    );
    // The waiter is told at the first exit, not after the attempts.
    let exited = Cause::Exited {
        code: ExitCode(1),
        tail: Tail::default(),
    };
    assert_eq!(supervised.want(&id).await, Err(Failed { cause: exited }));
    assert!(matches!(
        state(&supervised, &id),
        EngineState::Backoff { .. }
    ));
    // The machine goes on (2 s, then 4 s) and gives up after the third attempt.
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(
        state(&supervised, &id),
        EngineState::Failed(EngineFailure::Exited { code: ExitCode(1) })
    );
}

#[tokio::test(start_paused = true)]
async fn a_request_after_the_last_attempt_fails_at_once_and_none_restarts_it_early() {
    let scratch = Scratch::new("sup-pause");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let host = Recorder::default();
    let spawns = Arc::clone(&host.spawned);
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Exiting(host),
            probe: FakeReadyProbe(Probe::Down),
            gpu: gpu(16_000),
        },
    );
    assert!(supervised.want(&id).await.is_err());
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(matches!(
        state(&supervised, &id),
        EngineState::Failed(EngineFailure::Exited { .. })
    ));
    let tried = spawns.lock().expect("lock").len();
    assert_eq!(tried, 3, "the machine's three attempts");
    // Within the pause (30 s from the last failure) a request is failed without a spawn, however
    // often it asks.
    for _ in 0..5 {
        assert!(supervised.want(&id).await.is_err());
        supervised.warm(&id);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert_eq!(spawns.lock().expect("lock").len(), tried);
    // The pause runs out: the engine reads as tryable again and the next request starts it.
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(supervised.want(&id).await.is_err());
    assert_eq!(spawns.lock().expect("lock").len(), tried + 1);
}

/// A host whose every process ends the moment it is started.
#[derive(Debug, Clone)]
struct Exiting(Recorder);

impl EngineHost for Exiting {
    async fn spawn(
        &self,
        id: &EngineId,
        unit: &engine_supervisor::UnitSpec,
    ) -> Result<(), engine_supervisor::HostError> {
        self.0.spawn(id, unit).await
    }

    async fn stop(&self, id: &EngineId) -> Result<(), engine_supervisor::HostError> {
        self.0.stop(id).await
    }

    async fn exited(&self, _: &EngineId) -> ExitCode {
        ExitCode(7)
    }
}

/// A host that refuses to start anything, saying why through the diagnostics.
#[derive(Debug, Clone)]
struct Refusing(Diagnostics, Cause);

impl EngineHost for Refusing {
    async fn spawn(
        &self,
        id: &EngineId,
        _: &engine_supervisor::UnitSpec,
    ) -> Result<(), engine_supervisor::HostError> {
        self.0.refuse(id, self.1.clone());
        Err(engine_supervisor::HostError::Refused)
    }

    async fn stop(&self, _: &EngineId) -> Result<(), engine_supervisor::HostError> {
        Err(engine_supervisor::HostError::NotRunning)
    }

    async fn exited(&self, _: &EngineId) -> ExitCode {
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn a_refused_spawn_fails_the_waiter_with_the_hosts_reason_and_logs_it_at_error() {
    let scratch = Scratch::new("sup-refused");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let diagnostics = Diagnostics::default().logging_to(Log::to(move |level, text| {
        sink.lock().expect("lock").push((level, text.to_owned()));
    }));
    let cause = Cause::SocketPathTooLong {
        path: "/very/long".into(),
        len: 120,
    };
    let supervised = Supervised::start_with(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Refusing(diagnostics.clone(), cause.clone()),
            probe: FakeReadyProbe(Probe::Down),
            gpu: gpu(16_000),
        },
        diagnostics,
    );
    assert_eq!(supervised.want(&id).await, Err(Failed { cause }));
    let lines = lines.lock().expect("lock");
    assert_eq!(lines.first().map(|(level, _)| *level), Some(Level::Error));
    assert!(
        lines[0].1.contains("/very/long") && lines[0].1.contains("120"),
        "{lines:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_start_that_never_gets_ready_is_a_failure_when_its_time_is_up() {
    let scratch = Scratch::new("sup-never");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    // A probe that takes its time, as a real one does (the machine asks again at once after
    // each answer, so an instant one would never let the paused clock move).
    #[derive(Debug, Clone)]
    struct Slow;
    impl ReadyProbe for Slow {
        async fn probe(&self, _: &EngineId) -> Probe {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Probe::Loading
        }
    }
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: Slow,
            gpu: gpu(16_000),
        },
    );
    let failed = supervised.want(&id).await.expect_err("never ready");
    assert!(
        matches!(failed.cause, Cause::NeverReady { .. }),
        "{failed:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_model_that_does_not_fit_fails_with_the_numbers() {
    let scratch = Scratch::new("sup-room");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let need = specs[0].need;
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(500),
        },
    );
    assert_eq!(
        supervised.want(&id).await,
        Err(Failed {
            cause: Cause::NoRoom {
                need,
                free: MiB(500)
            }
        })
    );
    let EngineState::Failed(EngineFailure::NoRoom { need: asked, free }) = state(&supervised, &id)
    else {
        panic!("no room");
    };
    assert_eq!((asked, free), (need, MiB(500)));
}

#[tokio::test(start_paused = true)]
async fn a_second_model_evicts_the_idle_first_when_they_do_not_fit_together() {
    let scratch = Scratch::new("sup-evict");
    let mut specs = specs(&scratch);
    specs.truncate(2);
    specs.iter_mut().for_each(|spec| spec.need = MiB(9_000));
    let (a, b) = (specs[0].id.clone(), specs[1].id.clone());
    let host = Recorder::default();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: host.clone(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(supervised.want(&a).await, Ok(()));
    // Let the first engine go idle: a turn in the last probe window is never a victim.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(supervised.want(&b).await, Ok(()));
    assert_eq!(*host.stopped.lock().expect("lock"), vec![a.clone()]);
    assert!(matches!(state(&supervised, &a), EngineState::Stopped));
    assert!(matches!(state(&supervised, &b), EngineState::Ready { .. }));
}

#[tokio::test]
async fn a_supervisor_of_nothing_refuses_every_want_and_an_unknown_engine_too() {
    let none = Supervised::idle();
    assert_eq!(
        none.want(&EngineId("vllm:x".into())).await,
        Err(Failed::unknown())
    );
    assert_eq!(none.snapshot(), Snapshot::default());

    let scratch = Scratch::new("sup-unknown");
    let supervised = Supervised::start(
        specs(&scratch),
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(
        supervised.want(&EngineId("vllm:nobody".into())).await,
        Err(Failed::unknown())
    );
}

fn snapshot_at(now: u64, state: EngineState, gpu_total: u32) -> Snapshot {
    Snapshot {
        failures: BTreeMap::new(),
        states: [(EngineId("a".into()), state)].into(),
        now: Some(MonoMs(now)),
        gpu: Some(GpuMemory {
            total: MiB(gpu_total),
            used_by_others: MiB(0),
        }),
        ..Snapshot::default()
    }
}

#[test]
fn only_what_a_reader_sees_is_a_change() {
    let ready = EngineState::Ready {
        since: MonoMs(0),
        last_used: MonoMs(0),
    };
    let used = EngineState::Ready {
        since: MonoMs(0),
        last_used: MonoMs(5),
    };
    let mut failing = snapshot_at(10, EngineState::Stopped, 100);
    failing.failures.insert(
        EngineId("a".into()),
        Failure {
            cause: Cause::Unknown,
            seq: 1,
            retry_at: MonoMs(30),
        },
    );
    let mut later = failing.clone();
    later.now = Some(MonoMs(20));
    let mut after_pause = failing.clone();
    after_pause.now = Some(MonoMs(31));
    let cases = [
        (
            "the clock alone",
            snapshot_at(1, ready, 100),
            snapshot_at(900, ready, 100),
            Change::Unseen,
        ),
        (
            "an engine's state",
            snapshot_at(1, ready, 100),
            snapshot_at(2, used, 100),
            Change::Seen,
        ),
        (
            "the gpu",
            snapshot_at(1, ready, 100),
            snapshot_at(1, ready, 200),
            Change::Seen,
        ),
        (
            "a pause not over yet",
            failing.clone(),
            later,
            Change::Unseen,
        ),
        ("a pause that ends", failing, after_pause, Change::Seen),
    ];
    for (name, before, after, expected) in cases {
        assert_eq!(before.change_to(&after), expected, "{name}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_pump_that_changed_nothing_publishes_nothing_and_a_change_publishes_once() {
    let scratch = Scratch::new("sup-quiet");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: gpu(16_000),
        },
    );
    assert_eq!(supervised.want(&id).await, Ok(()));
    let mut watching = supervised.state.clone();
    watching.borrow_and_update();
    // The wakes in the next 100 s pump the machine; nothing in it changes.
    let quiet = tokio::time::timeout(Duration::from_secs(100), watching.changed()).await;
    assert!(quiet.is_err(), "a pump with nothing changed published");
    // The idle unload at 600 s is a change, and it is published.
    watching.changed().await.expect("the unload is published");
}

#[tokio::test(start_paused = true)]
async fn a_probe_that_fails_at_once_is_asked_again_only_when_its_turn_comes() {
    struct Counting(Arc<Mutex<u32>>);
    impl ReadyProbe for Counting {
        async fn probe(&self, _: &EngineId) -> Probe {
            *self.0.lock().expect("lock") += 1;
            Probe::Down
        }
    }
    let scratch = Scratch::new("sup-hot");
    let specs = specs(&scratch);
    let id = specs[0].id.clone();
    let asked = Arc::new(Mutex::new(0));
    let supervised = Supervised::start(
        specs,
        SupervisorConfig::default(),
        Ports {
            host: Recorder::default(),
            probe: Counting(Arc::clone(&asked)),
            gpu: gpu(16_000),
        },
    );
    supervised.warm(&id);
    tokio::time::sleep(Duration::from_secs(10)).await;
    // One probe every 500 ms, not one per scheduler turn.
    assert!(*asked.lock().expect("lock") <= 25);
}
