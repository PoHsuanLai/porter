//! A failure ends only the session it hit: one session's task panics and its client is told in
//! plain words, while a second session on the same daemon goes on and completes.

use crate::serve::{chat, mail};
use crate::support::*;
use inferd::serve::{guarded, serve_session};
use porter_infer::{InferEvent, InferReply, InferRequest, ModelError};
use porter_router::seams::{Seams, TurnStep};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream as StdStream;
use tokio::net::UnixStream;

/// A runner whose turns blow up the moment they start.
#[derive(Debug, Default)]
struct Exploding;

impl porter_router::seams::TurnRunner for Exploding {
    type Turn = Turn;

    fn start(&self, _request: InferRequest, _attachments: Vec<OwnedFd>) -> Turn {
        panic!("a turn that blows up, on purpose");
    }

    fn start_heard(
        &self,
        _chat: porter_infer::ChatRequest,
        _heard: porter_router::session::HeardAudio,
    ) -> Turn {
        panic!("a turn that blows up, on purpose");
    }
}

/// The daemon's end of a session socket, the handle it reports on, and the client.
fn socket() -> (UnixStream, UnixStream, Client) {
    let (ours, theirs) = StdStream::pair().expect("pair");
    ours.set_nonblocking(true).expect("nonblocking");
    theirs.set_nonblocking(true).expect("nonblocking");
    let report = UnixStream::from_std(ours.try_clone().expect("clone")).expect("report");
    let stream = UnixStream::from_std(ours).expect("stream");
    let client = Client::new(UnixStream::from_std(theirs).expect("client"));
    (stream, report, client)
}

#[tokio::test]
async fn a_session_that_panics_is_told_so_and_a_second_session_completes() {
    // The healthy session first, idle, so both are alive together.
    let (stream, report, mut healthy) = socket();
    let runner = Runner::default();
    let seams = Seams {
        router: FixedRouter::ready(),
        engines: Engines::default(),
        runner: runner.clone(),
        audit: Audit::default(),
    };
    tokio::spawn(guarded(
        async move { serve_session(stream, mail(), &seams).await },
        report,
        || "test".to_owned(),
    ));
    healthy.send(&chat(), &[]).await;
    let waiting = runner.clone();
    eventually("the healthy turn started", move || {
        waiting.started.lock().expect("lock").len() == 1
    })
    .await;

    // The second session panics when its turn starts.
    let (stream, report, mut doomed) = socket();
    let seams = Seams {
        router: FixedRouter::ready(),
        engines: Engines::default(),
        runner: Exploding,
        audit: Audit::default(),
    };
    tokio::spawn(guarded(
        async move { serve_session(stream, mail(), &seams).await },
        report,
        || "test".to_owned(),
    ));
    doomed.send(&chat(), &[]).await;
    let told = doomed.until_finished().await;
    assert_eq!(
        told.last(),
        Some(&InferEvent::Finished(InferReply::Failed(
            ModelError::Interrupted
        )))
    );

    // The first session was not touched: its turn still answers.
    runner.started.lock().expect("lock")[0]
        .tx
        .send(TurnStep::Done(InferReply::Cancelled))
        .expect("the turn is alive");
    let events = healthy.until_finished().await;
    assert_eq!(
        events.last(),
        Some(&InferEvent::Finished(InferReply::Cancelled))
    );
}
