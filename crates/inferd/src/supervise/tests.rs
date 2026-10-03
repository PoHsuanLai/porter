//! The driver over stoker's seams, with the clock paused: time moves only when every task waits,
//! so backoffs and the idle timer cost nothing.

use super::*;
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
    assert_eq!(supervised.want(&id).await, Err(Failed));
    assert_eq!(
        state(&supervised, &id),
        EngineState::Failed(EngineFailure::Exited { code: ExitCode(1) })
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
    assert_eq!(supervised.want(&id).await, Err(Failed));
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
    assert_eq!(none.want(&EngineId("vllm:x".into())).await, Err(Failed));
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
        Err(Failed)
    );
}
