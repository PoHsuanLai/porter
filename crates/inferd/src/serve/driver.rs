//! The loop of one session: feed the machine (`session::step`) what happens, carry out what it
//! asks. The machine decides; this file only moves bytes, futures and descriptors.

use super::carried::Tally;
use super::seams::{
    AuditSink, EngineFailed, EngineHost, Router, RunningTurn, Seams, TurnRunner, TurnStep,
};
use super::wire::Wire;
use crate::session::{Phase, SessionIn, SessionOut, SessionSpec, step};
use porter_infer::{ClientFrame, ServedBy, Why};
use std::future::Future;
use std::os::fd::OwnedFd;
use std::pin::Pin;
use tokio::net::UnixStream;

type Wait<'a> = Pin<Box<dyn Future<Output = Result<(), EngineFailed>> + Send + 'a>>;

/// Everything one session holds while it runs.
struct Live<'a, E: EngineHost, T: TurnRunner> {
    phase: Phase,
    turn: Option<T::Turn>,
    wait: Option<Wait<'a>>,
    /// The descriptors of the one request that may be queued, until it starts.
    held: Option<Vec<OwnedFd>>,
    /// What the turn in flight carries, for the audit entry.
    tally: Tally,
    /// Why the route chose the model the session is pinned to, for the audit entry.
    why: Why,
    engines: &'a E,
}

/// Serves one `Open` socket until the client leaves, the session closes or the wire breaks.
pub async fn serve_session<R, E, T, A>(
    stream: UnixStream,
    spec: SessionSpec,
    seams: &Seams<R, E, T, A>,
) where
    R: Router,
    E: EngineHost,
    T: TurnRunner,
    A: AuditSink,
{
    let mut wire = Wire::new(stream);
    let mut live: Live<'_, E, T> = Live {
        phase: Phase::Opened,
        turn: None,
        wait: None,
        held: None,
        tally: Tally::default(),
        why: Why::Named,
        engines: &seams.engines,
    };
    let routed = seams.router.route(&spec).await;
    if let Ok(decision) = &routed {
        live.why = decision.why.clone();
    }
    let mut input = SessionIn::Routed(routed);
    let mut incoming: Vec<OwnedFd> = Vec::new();
    loop {
        if !advance(
            &mut live,
            &mut wire,
            &spec,
            seams,
            input,
            std::mem::take(&mut incoming),
        )
        .await
        {
            return;
        }
        input = match next_input(&mut live, &mut wire, &mut incoming).await {
            Some(input) => input,
            None => SessionIn::Closed,
        };
    }
}

/// Waits for the next thing that happens to the session.
async fn next_input<E: EngineHost, T: TurnRunner>(
    live: &mut Live<'_, E, T>,
    wire: &mut Wire,
    incoming: &mut Vec<OwnedFd>,
) -> Option<SessionIn> {
    let Live { turn, wait, .. } = live;
    tokio::select! {
        read = wire.next_frame() => match read {
            Ok(Some((frame, fds))) => {
                *incoming = fds;
                Some(SessionIn::Frame(frame))
            }
            Ok(None) | Err(_) => None,
        },
        step = async { match turn { Some(turn) => turn.next().await, None => std::future::pending().await } }, if turn.is_some() => {
            Some(match step {
                TurnStep::Event(event) => SessionIn::TurnEvent(event),
                TurnStep::Done(reply) => SessionIn::TurnDone(reply),
            })
        },
        ready = async { match wait { Some(wait) => wait.await, None => std::future::pending().await } }, if wait.is_some() => {
            *wait = None;
            Some(match ready {
                Ok(()) => SessionIn::EngineReady,
                Err(EngineFailed) => SessionIn::EngineFailed,
            })
        },
    }
}

/// One transition and its effects; false once the session is over.
async fn advance<'a, R, E, T, A>(
    live: &mut Live<'a, E, T>,
    wire: &mut Wire,
    spec: &SessionSpec,
    seams: &'a Seams<R, E, T, A>,
    input: SessionIn,
    incoming: Vec<OwnedFd>,
) -> bool
where
    R: Router,
    E: EngineHost,
    T: TurnRunner,
    A: AuditSink,
{
    let before = std::mem::replace(&mut live.phase, Phase::Closed);
    let served = served_of(&before);
    let fresh_request = matches!(&input, SessionIn::Frame(ClientFrame::Request(_)));
    let started_by_frame = fresh_request && matches!(before, Phase::Idle { .. });
    let had_queued = queued_of(&before);
    let (after, effects) = step(spec, before, input);
    let queued_now = fresh_request && !had_queued && queued_of(&after);
    live.phase = after;
    // A request that queues keeps its descriptors until it starts; one that starts at once
    // hands them to its turn; one that is refused drops them (closing them).
    let own = match (queued_now, started_by_frame) {
        (true, _) => {
            live.held = Some(incoming);
            None
        }
        (false, true) => Some(incoming),
        (false, false) => None,
    };
    run_effects(live, wire, spec, seams, served, effects, own).await
}

async fn run_effects<'a, R, E, T, A>(
    live: &mut Live<'a, E, T>,
    wire: &mut Wire,
    spec: &SessionSpec,
    seams: &'a Seams<R, E, T, A>,
    served: Option<ServedBy>,
    effects: Vec<SessionOut>,
    mut from_frame: Option<Vec<OwnedFd>>,
) -> bool
where
    R: Router,
    E: EngineHost,
    T: TurnRunner,
    A: AuditSink,
{
    for effect in effects {
        match effect {
            SessionOut::Want(model) => live.wait = Some(Box::pin(seams.engines.want(model))),
            SessionOut::Release(model) => {
                live.wait = None;
                live.engines.release(&model);
            }
            SessionOut::Emit(event) => {
                if wire.write(&event).await.is_err() {
                    return false;
                }
            }
            SessionOut::StartTurn(request) => {
                let fds = from_frame
                    .take()
                    .or_else(|| live.held.take())
                    .unwrap_or_default();
                live.tally = Tally::begin(&request);
                live.turn = Some(seams.runner.start(request, fds));
            }
            SessionOut::DropTurn => live.turn = None,
            SessionOut::Audio(frame) => {
                live.tally.heard(&frame);
                if let Some(turn) = live.turn.as_mut() {
                    turn.audio(frame);
                }
            }
            SessionOut::EndAudio => {
                if let Some(turn) = live.turn.as_mut() {
                    turn.end_audio();
                }
            }
            SessionOut::Audit(reply) => {
                if let Some(served) = &served {
                    let carried = live.tally.closing(&reply);
                    seams
                        .audit
                        .record(spec, served, &reply, &carried, &live.why);
                }
            }
        }
    }
    !matches!(live.phase, Phase::Closed)
}

fn served_of(phase: &Phase) -> Option<ServedBy> {
    match phase {
        Phase::Waiting { served, .. }
        | Phase::Idle { served, .. }
        | Phase::InTurn { served, .. } => Some(served.clone()),
        Phase::Opened | Phase::Closed => None,
    }
}

fn queued_of(phase: &Phase) -> bool {
    matches!(
        phase,
        Phase::Waiting {
            queued: Some(_),
            ..
        } | Phase::InTurn {
            queued: Some(_),
            ..
        }
    )
}
