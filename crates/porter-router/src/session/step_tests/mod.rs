//! The rows of models §4.2 and voice §3.4 as tables over `step`, then the invariants that hold
//! for any input.

mod helpers;
mod rows_cua;
mod rows_hear;
mod rows_queue;
mod rows_route;
mod rows_turn;
mod walk;

use helpers::*;
use porter_core::DataClass;
use porter_infer::{ClientFrame, InferEvent, InferReply, Readiness, RequestKind};

fn rows() -> Vec<Row> {
    [
        rows_route::rows(),
        rows_turn::rows(),
        rows_queue::rows(),
        rows_cua::rows(),
    ]
    .concat()
}

#[test]
fn session_transitions() {
    for (name, spec, phase, input, next, out) in rows() {
        let (got, effects) = step(&spec, phase, input);
        assert_eq!(got, next, "{name}: phase");
        assert_eq!(effects, out, "{name}: effects");
    }
}

#[test]
fn every_turn_ends_with_exactly_one_finished() {
    let spec = speech_spec(DataClass::Voice);
    let inputs = [
        frame(ClientFrame::Cancel),
        SessionIn::TurnDone(InferReply::Cancelled),
        frame(audio(9, 8)),
        SessionIn::EngineFailed,
    ];
    for input in inputs {
        let (_, effects) = step(
            &spec,
            in_turn(RequestKind::Transcribe, expecting(0)),
            input.clone(),
        );
        let finished = effects
            .iter()
            .filter(|e| matches!(e, SessionOut::Emit(InferEvent::Finished(_))))
            .count();
        assert_eq!(finished, 1, "{input:?}");
    }
}

#[test]
fn a_closed_session_never_acts_again() {
    let spec = llm_spec(DataClass::Mail);
    let inputs = [
        SessionIn::EngineReady,
        decided(Readiness::Ready),
        frame(ClientFrame::Cancel),
        SessionIn::Closed,
    ];
    for input in inputs {
        assert_eq!(
            step(&spec, Phase::Closed, input.clone()),
            (Phase::Closed, vec![]),
            "{input:?}"
        );
    }
}
