use super::*;
use crate::replay::{NamedEngine, Replays};
use crate::supervise::{Ports, Supervised};
use engine_supervisor::{FakeReadyProbe, GpuError, GpuMemory, GpuProbe, Probe, SupervisorConfig};

/// A computer with no `nvidia-smi`.
struct NoGpu;

impl GpuProbe for NoGpu {
    async fn memory(&self) -> Result<GpuMemory, GpuError> {
        Err(GpuError::Unavailable)
    }
}

struct Never;

impl EngineHost for Never {
    async fn spawn(&self, _: &EngineId, _: &UnitSpec) -> Result<(), HostError> {
        Err(HostError::Refused)
    }
    async fn stop(&self, _: &EngineId) -> Result<(), HostError> {
        Err(HostError::NotRunning)
    }
    async fn exited(&self, _: &EngineId) -> ExitCode {
        std::future::pending().await
    }
}

fn cassette(dir: &crate::testkit::Scratch) -> std::path::PathBuf {
    let path = dir.path().join("c.jsonl");
    let header = crate::replay::cassette::TEST_HEADER;
    std::fs::write(&path, format!("{header}\n")).expect("write");
    path
}

#[tokio::test]
async fn a_replay_only_config_with_no_gpu_starts_and_serves() {
    let dir = crate::testkit::Scratch::new("replay-nogpu");
    let named = [(
        "scripted".to_owned(),
        NamedEngine {
            replay: cassette(&dir),
        },
    )]
    .into();
    let replays = Replays::read(&named, dir.path());
    let id = replays.models[0].spec.id.clone();
    let supervised = Supervised::start(
        vec![replays.models[0].spec.clone()],
        replays.supervisor_config(1),
        Ports {
            host: replays.host(Never),
            probe: FakeReadyProbe(Probe::Ready),
            gpu: NoGpu,
        },
    );
    assert_eq!(supervised.want(&id).await, Ok(()));
    assert!(replays.models[0].socket.0.exists(), "the engine listens");
}

#[tokio::test]
async fn the_headroom_is_kept_when_a_real_engine_is_among_the_models() {
    let replays = Replays::default();
    assert_eq!(replays.supervisor_config(0), SupervisorConfig::default());
    let dir = crate::testkit::Scratch::new("replay-mixed");
    let named = [(
        "scripted".to_owned(),
        NamedEngine {
            replay: cassette(&dir),
        },
    )]
    .into();
    let replays = Replays::read(&named, dir.path());
    assert_eq!(replays.supervisor_config(2), SupervisorConfig::default());
}
