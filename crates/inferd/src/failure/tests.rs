use super::*;
use porter_core::Count;

fn memory(free: u64, one_more: u64) -> Option<MemoryFacts> {
    Some(MemoryFacts {
        free: Bytes(free),
        one_more: Bytes(one_more),
    })
}

#[test]
fn an_unreachable_engine_is_told_as_stopped_or_as_out_of_memory_and_nothing_else_changes() {
    let cases = [
        (
            ModelError::Unreachable,
            EngineView::NotRunning,
            memory(10, 1),
            ModelError::EngineStopped,
        ),
        (
            ModelError::Unreachable,
            EngineView::Running,
            memory(1, 10),
            ModelError::OutOfMemory,
        ),
        // enough memory, or none read: the error stands
        (
            ModelError::Unreachable,
            EngineView::Running,
            memory(10, 10),
            ModelError::Unreachable,
        ),
        (
            ModelError::Unreachable,
            EngineView::Running,
            None,
            ModelError::Unreachable,
        ),
        // a refused prompt, a restart in progress and a rate limit are not the engine dying
        (
            ModelError::ContextOverflow,
            EngineView::NotRunning,
            memory(1, 10),
            ModelError::ContextOverflow,
        ),
        (
            ModelError::NotReady,
            EngineView::NotRunning,
            None,
            ModelError::NotReady,
        ),
        (
            ModelError::RateLimited(3),
            EngineView::Running,
            memory(1, 10),
            ModelError::RateLimited(3),
        ),
    ];
    for (error, engine, memory, expected) in cases {
        assert_eq!(explain(error, &Moment { engine, memory }), expected);
    }
}

#[test]
fn only_the_persons_unspoken_request_on_a_stopped_engine_moves_once_and_only_when_there_is_somewhere()
 {
    let go = |who, error, spoke, tried, others| retry(who, error, spoke, tried, others);
    let stopped = ModelError::EngineStopped;
    assert_eq!(
        go(Priority::Person, stopped, Spoke::Nothing, Tried::Never, 1),
        Retry::Elsewhere
    );
    let refused = [
        go(Priority::Helper, stopped, Spoke::Nothing, Tried::Never, 1),
        go(Priority::Person, stopped, Spoke::Something, Tried::Never, 1),
        go(Priority::Person, stopped, Spoke::Nothing, Tried::Once, 1),
        go(Priority::Person, stopped, Spoke::Nothing, Tried::Never, 0),
        go(
            Priority::Person,
            ModelError::OutOfMemory,
            Spoke::Nothing,
            Tried::Never,
            1,
        ),
        go(
            Priority::Person,
            ModelError::ContextOverflow,
            Spoke::Nothing,
            Tried::Never,
            1,
        ),
    ];
    assert!(refused.iter().all(|one| *one == Retry::No), "{refused:?}");
}

#[test]
fn the_log_line_carries_the_numbers_of_the_moment() {
    let facts = HardwareFacts {
        helpers: Count(3),
        ahead: Count(2),
        memory: memory(512 << 20, 2048 << 20),
    };
    let line = log_line(
        "a reply failed",
        Some(&EngineId("vllm:qwen".into())),
        &facts,
    );
    assert_eq!(
        line,
        "inferd: a reply failed on vllm:qwen: graphics memory 512 MiB free, about 2048 MiB for one more; 3 helpers using it, 2 waiting"
    );
    let blind = log_line("a session failed", None, &HardwareFacts::default());
    assert!(blind.contains("graphics memory not read"), "{blind}");
}
