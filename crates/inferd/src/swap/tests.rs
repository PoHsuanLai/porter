use super::*;
use engine_supervisor::{GpuAccess, Network, ProgramPath, Sandbox, UnitSpec};
use model_catalog::MiB;
use porter_core::{AccountId, ModelId};
use std::path::PathBuf;

fn id(name: &str) -> EngineId {
    EngineId(name.to_owned())
}

fn spec(name: &str, need: u32) -> EngineSpec {
    EngineSpec {
        id: id(name),
        need: MiB(need),
        unit: UnitSpec {
            program: ProgramPath(PathBuf::from("/usr/bin/engine")),
            args: Vec::new(),
            env: Vec::new(),
            sandbox: Sandbox {
                network: Network::None,
                read: Vec::new(),
                write: Vec::new(),
                gpu: GpuAccess::Nvidia,
                memory_max: MiB(need),
            },
        },
    }
}

fn model(name: &str) -> Option<ModelRef> {
    Some(ModelRef {
        account: AccountId::parse("local").expect("id"),
        model: ModelId::parse(name).expect("id"),
    })
}

const NOW: MonoMs = MonoMs(100_000);

fn budget_now(gpu: Option<GpuMemory>) -> Budget {
    Budget {
        gpu,
        headroom: MiB(1000),
        now: NOW,
        probe_every: Duration::from_millis(500),
    }
}

const GPU: GpuMemory = GpuMemory {
    total: MiB(16000),
    used_by_others: MiB(0),
};

fn run(name: &str, mem: u32, last: u64) -> (EngineId, MiB, MonoMs) {
    (id(name), MiB(mem), MonoMs(last))
}

fn cost(want: u32, running: &Running, gpu: Option<GpuMemory>) -> SwapCost {
    swap_cost(
        &spec("w", want),
        running,
        budget_now(gpu),
        |e| model(&e.0),
        90,
    )
}

#[test]
fn an_unobserved_gpu_is_not_in_the_way() {
    assert_eq!(
        cost(99_999, &vec![], None),
        SwapCost::Fits {
            cold_start_estimate_s: 90
        }
    );
}

#[test]
fn it_fits_beside_what_runs() {
    let running = vec![run("a", 4000, 1)];
    assert_eq!(
        cost(8000, &running, Some(GPU)),
        SwapCost::Fits {
            cold_start_estimate_s: 90
        }
    );
}

#[test]
fn one_idle_engine_is_the_named_victim() {
    let running = vec![run("a", 10_000, 1)];
    assert_eq!(
        cost(8000, &running, Some(GPU)),
        SwapCost::Evicts {
            victim: model("a").expect("ref"),
            load: EngineLoad::Idle,
            cold_start_estimate_s: 90
        }
    );
}

#[test]
fn an_engine_in_a_turn_is_never_the_victim() {
    // `a` was used 100 ms ago: in a turn. The only way to make room is to unload it, so there is
    // no room; with an idle engine beside it, the idle one is named and the busy one stays.
    let busy = run("busy", 10_000, NOW.0 - 100);
    assert_eq!(cost(8000, &vec![busy.clone()], Some(GPU)), SwapCost::NoRoom);
    let idle = run("idle", 4000, 1);
    let got = cost(5000, &vec![busy, idle], Some(GPU));
    assert_eq!(
        got,
        SwapCost::Evicts {
            victim: model("idle").expect("ref"),
            load: EngineLoad::Idle,
            cold_start_estimate_s: 90
        }
    );
}

#[test]
fn several_victims_are_a_purge_and_a_stranger_is_no_room() {
    let running = vec![run("a", 6000, 1), run("b", 6000, 2)];
    assert_eq!(
        cost(12_000, &running, Some(GPU)),
        SwapCost::Purge { engines: 2 }
    );
    let strange = swap_cost(
        &spec("w", 8000),
        &vec![run("x", 10_000, 1)],
        budget_now(Some(GPU)),
        |_| None,
        90,
    );
    assert_eq!(strange, SwapCost::NoRoom);
}

#[test]
fn only_ready_engines_hold_memory() {
    let states = [
        (
            id("a"),
            EngineState::Ready {
                since: MonoMs(1),
                last_used: MonoMs(5),
            },
        ),
        (id("b"), EngineState::Stopped),
    ];
    let got = running_of(states.iter().map(|(i, s)| (i, s)), |_| Some(MiB(100)));
    assert_eq!(got, vec![run("a", 100, 5)]);
}
