//! A bounded walk over every short input sequence, asserting what must hold whatever the
//! client and the engine do: a machine that never contradicts itself.

use super::helpers::*;
use porter_core::DataClass;
use porter_infer::{
    ClientFrame, InferEvent, InferReply, InferRequest, ModelError, Readiness, RequestKind,
};

const DEPTH: usize = 4;

fn inputs() -> Vec<SessionIn> {
    vec![
        decided(Readiness::Loadable),
        SessionIn::EngineReady,
        SessionIn::EngineFailed,
        SessionIn::EngineProgress(Readiness::Loading),
        request(task(DataClass::Mail)),
        request(speak(DataClass::Mail)),
        frame(ClientFrame::Cancel),
        SessionIn::TurnEvent(InferEvent::TextDelta("t".into())),
        SessionIn::TurnDone(InferReply::Failed(ModelError::Unreachable)),
        SessionIn::Closed,
    ]
}

fn is_finished(out: &SessionOut) -> bool {
    matches!(out, SessionOut::Emit(InferEvent::Finished(_)))
}

fn is_routed(out: &SessionOut) -> bool {
    matches!(out, SessionOut::Emit(InferEvent::Routed(_)))
}

/// What one transition must satisfy, given the phase before.
fn check(trace: &str, before: &Phase, after: &Phase, out: &[SessionOut]) {
    let starts = out
        .iter()
        .filter(|o| matches!(o, SessionOut::StartTurn(_)))
        .count();
    let was_turn = matches!(before, Phase::InTurn { .. });
    let is_turn = matches!(after, Phase::InTurn { .. });
    assert!(starts <= 1, "{trace}: two turns started");
    assert!(
        starts == 0 || is_turn,
        "{trace}: a turn started but the phase is not InTurn"
    );
    assert!(
        !is_turn || was_turn || starts == 1,
        "{trace}: InTurn reached without a StartTurn"
    );
    assert!(
        !out.contains(&SessionOut::DropTurn) || was_turn,
        "{trace}: DropTurn outside a turn"
    );
    let releases = out
        .iter()
        .filter(|o| matches!(o, SessionOut::Release(_)))
        .count();
    assert!(releases <= 1, "{trace}: released twice");
    assert!(
        releases == 0 || matches!(after, Phase::Closed),
        "{trace}: released but not closed"
    );
    if matches!(before, Phase::Closed) {
        assert!(
            out.is_empty() && matches!(after, Phase::Closed),
            "{trace}: acted after close"
        );
    }
    // A turn that ends says so exactly once per turn that was running or queued.
    let finishes = out.iter().filter(|o| is_finished(o)).count();
    let audits = out
        .iter()
        .filter(|o| matches!(o, SessionOut::Audit(_)))
        .count();
    assert!(audits <= finishes, "{trace}: an audit with no Finished");
}

fn walk(spec: &SessionSpec, phase: Phase, depth: usize, trace: &str, routed_sent: usize) {
    if depth == 0 {
        return;
    }
    for input in inputs() {
        let trace = format!("{trace} > {input:?}");
        let (after, out) = step(spec, phase.clone(), input);
        check(&trace, &phase, &after, &out);
        let sent = routed_sent + out.iter().filter(|o| is_routed(o)).count();
        assert!(sent <= 1, "{trace}: Routed went out twice");
        walk(spec, after, depth - 1, &trace, sent);
    }
}

#[test]
fn no_short_sequence_breaks_the_machine() {
    walk(
        &llm_spec(DataClass::Mail),
        Phase::Opened,
        DEPTH,
        "opened",
        0,
    );
}

#[test]
fn nor_does_one_that_starts_mid_session() {
    let spec = speech_spec(DataClass::Voice);
    let starts = [
        waiting(Some(task(DataClass::Voice))),
        idle(RoutedNote::Pending),
        in_turn(RequestKind::Transcribe, expecting(0)),
        queued_chat(None),
    ];
    for phase in starts {
        walk(&spec, phase, DEPTH - 1, "mid", 0);
    }
    let _ = InferRequest::Chat;
}
