use super::*;

fn engine() -> EngineId {
    EngineId("vllm:m".into())
}

fn args(words: &[&str]) -> Vec<EngineArg> {
    words.iter().map(|w| EngineArg((*w).to_owned())).collect()
}

fn helper() -> Asker {
    Asker::from_text("helper:h-1").expect("helper")
}

fn waiting(ahead: u32) -> Standing {
    Standing::Waiting {
        ahead: Count(ahead),
    }
}

#[test]
fn the_seats_are_read_from_the_launch_arguments() {
    let cases: Vec<(EngineKind, Vec<EngineArg>, Seats)> = vec![
        (
            EngineKind::Vllm,
            args(&["--max-num-seqs", "4"]),
            Seats::of(4),
        ),
        (EngineKind::Vllm, args(&["--max-num-seqs=8"]), Seats::of(8)),
        (
            EngineKind::LlamaServer,
            args(&["--ctx-size", "8192", "--parallel", "2"]),
            Seats::of(2),
        ),
        (EngineKind::LlamaServer, args(&["-np", "3"]), Seats::of(3)),
        // not said, not a number, the other engine's word, or not a chat engine: one
        (
            EngineKind::Vllm,
            args(&["--max-model-len", "8192"]),
            Seats::of(1),
        ),
        (
            EngineKind::Vllm,
            args(&["--max-num-seqs", "many"]),
            Seats::of(1),
        ),
        (EngineKind::Vllm, args(&["--parallel", "4"]), Seats::of(1)),
        (EngineKind::Vllm, args(&["--max-num-seqs"]), Seats::of(1)),
        (
            EngineKind::SpeechHost,
            args(&["--parallel", "4"]),
            Seats::of(1),
        ),
    ];
    for (kind, given, expected) in cases {
        assert_eq!(seats_in(kind, &given), expected, "{kind:?} {given:?}");
    }
}

#[test]
fn the_person_is_seated_before_helpers_and_the_queue_numbers_follow() {
    let gate = Gate::default();
    let seats = Seats::of(1);
    let running = gate.pass(&engine(), seats, &helper());
    let first = gate.pass(&engine(), seats, &helper());
    let second = gate.pass(&engine(), seats, &helper());
    assert_eq!(
        [running.standing(), first.standing(), second.standing()],
        [Standing::Seated, waiting(0), waiting(1)]
    );
    let person = gate.pass(&engine(), seats, &Asker::Person);
    assert_eq!(person.standing(), waiting(0));
    assert_eq!(
        (first.standing(), second.standing()),
        (waiting(1), waiting(2))
    );
    drop(running);
    assert_eq!(person.standing(), Standing::Seated);
    assert_eq!(
        (first.standing(), second.standing()),
        (waiting(0), waiting(1))
    );
    // A request that gives up while it waits closes the gap behind it.
    drop(first);
    assert_eq!(second.standing(), waiting(0));
    drop(person);
    assert_eq!(second.standing(), Standing::Seated);
}

#[tokio::test]
async fn a_waiting_request_is_woken_when_its_turn_comes() {
    let gate = Gate::default();
    let holder = gate.pass(&engine(), Seats::of(1), &Asker::Person);
    let mut waits = gate.pass(&engine(), Seats::of(1), &helper());
    assert_eq!(waits.standing(), waiting(0));
    let woken = tokio::spawn(async move {
        while waits.standing() != Standing::Seated {
            if !waits.moved().await {
                return false;
            }
        }
        true
    });
    drop(holder);
    assert!(woken.await.expect("task"));
}

#[test]
fn an_aborted_request_frees_its_seat() {
    let gate = Gate::default();
    let seats = Seats::of(1);
    let first = gate.pass(&engine(), seats, &Asker::Person);
    let next = gate.pass(&engine(), seats, &helper());
    drop(first);
    assert_eq!(next.standing(), Standing::Seated);
    drop(next);
    assert_eq!(
        gate.pass(&engine(), seats, &helper()).standing(),
        Standing::Seated
    );
}
