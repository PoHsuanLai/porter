//! The session an [`EngineHost`](super::EngineHost) opens: the route is chosen once, at `open`
//! (so the session is pinned to one engine), and a refusal is its first event, as on the bus.
//! A request starts a turn on a task of the runtime; its events arrive through `next`, the
//! reply last (`Finished`). `Routed` goes out once, with the first request, as inferd's does.
//!
//! A session carries one turn at a time: a request while one runs is a `Malformed` error, and
//! `Cancel` ends the running turn with `Finished(Cancelled)`. Dropping the session ends the turn
//! (the HTTP future is dropped, which closes the engine's stream). Like the daemon's, a session
//! with no request outstanding waits in `next`.

use super::keys::KeySource;
use super::turn::{Pinned, reply};
use porter_bridge::Frames;
use porter_core::DataClass;
use porter_infer::{
    ClientFrame, InferEvent, InferRefusal, InferReply, InferRequest, InferSession, SessionError,
};
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

/// Where a session stands.
#[derive(Debug)]
enum Phase {
    /// Refused at open: the first `next` says so, and there is nothing after it.
    Refused(Option<InferRefusal>),
    /// Pinned to an engine, with no turn running.
    Idle(Pinned),
    /// A turn is running on this task.
    Running(Pinned, Turn),
}

/// A running turn, with the channel that is its own: ended with the session, or when it is
/// cancelled. Dropping it aborts the task and drops the receiver, so whatever the task still
/// sends (the abort lands at its next await, and on another thread it may be mid-send) goes
/// nowhere and can never reach the next turn.
#[derive(Debug)]
struct Turn {
    task: JoinHandle<()>,
    inbox: UnboundedReceiver<InferEvent>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Whether the `Routed` event has gone out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoutedNote {
    Pending,
    Sent,
}

/// A streaming session over the engines of an [`EngineHost`](super::EngineHost).
#[derive(Debug)]
pub struct EngineSession<K> {
    keys: Arc<K>,
    class: DataClass,
    phase: Phase,
    routed: RoutedNote,
    /// The session's own events (`Routed`, a refusal of a request, `Cancelled`); a turn's are on
    /// its own channel.
    events: UnboundedSender<InferEvent>,
    inbox: UnboundedReceiver<InferEvent>,
}

impl<K: KeySource + 'static> EngineSession<K> {
    pub(crate) fn refused(keys: Arc<K>, class: DataClass, refusal: InferRefusal) -> Self {
        Self::in_phase(keys, class, Phase::Refused(Some(refusal)))
    }

    pub(crate) fn pinned(keys: Arc<K>, class: DataClass, pinned: Pinned) -> Self {
        Self::in_phase(keys, class, Phase::Idle(pinned))
    }

    fn in_phase(keys: Arc<K>, class: DataClass, phase: Phase) -> Self {
        let (events, inbox) = unbounded_channel();
        Self {
            keys,
            class,
            phase,
            routed: RoutedNote::Pending,
            events,
            inbox,
        }
    }

    /// One frame, with the bytes of the images that rode on it by attach index.
    async fn send_with(&mut self, frame: ClientFrame, frames: Frames) -> Result<(), SessionError> {
        let request = match frame {
            ClientFrame::Request(request) => request,
            ClientFrame::Cancel => {
                self.cancel();
                return Ok(());
            }
            ClientFrame::Audio(_) | ClientFrame::EndOfAudio => {
                return Err(SessionError::Malformed(
                    "this host serves no audio".to_owned(),
                ));
            }
        };
        let phase = std::mem::replace(&mut self.phase, Phase::Refused(None));
        self.phase = match phase {
            // A refused session takes the request and says nothing more than the refusal.
            refused @ Phase::Refused(_) => refused,
            running @ Phase::Running(..) => {
                self.phase = running;
                return Err(SessionError::Malformed("a turn is running".to_owned()));
            }
            Phase::Idle(pinned) => self.start(pinned, request, frames),
        };
        Ok(())
    }

    /// Starts the turn for `request`, or answers at once when the session may not carry it.
    fn start(&mut self, pinned: Pinned, request: InferRequest, frames: Frames) -> Phase {
        if self.routed == RoutedNote::Pending {
            self.routed = RoutedNote::Sent;
            let _ = self.events.send(InferEvent::Routed(pinned.served.clone()));
        }
        if class_of(&request).is_some_and(|class| class != self.class) {
            let refusal = InferReply::Refused(InferRefusal::Unsupported);
            let _ = self.events.send(InferEvent::Finished(refusal));
            return Phase::Idle(pinned);
        }
        let (events, inbox) = unbounded_channel();
        let (keys, on) = (Arc::clone(&self.keys), pinned.clone());
        let task = tokio::spawn(async move {
            let answer = reply(&on, &*keys, &request, &frames, &events).await;
            let _ = events.send(InferEvent::Finished(answer));
        });
        Phase::Running(pinned, Turn { task, inbox })
    }

    /// Ends the running turn, if any, and says so.
    fn cancel(&mut self) {
        let phase = std::mem::replace(&mut self.phase, Phase::Refused(None));
        self.phase = match phase {
            Phase::Running(pinned, turn) => {
                drop(turn);
                let _ = self
                    .events
                    .send(InferEvent::Finished(InferReply::Cancelled));
                Phase::Idle(pinned)
            }
            other => other,
        };
    }
}

/// The data class a request carries; everything on a session has the class it opened with.
fn class_of(request: &InferRequest) -> Option<DataClass> {
    match request {
        InferRequest::Chat(chat) => Some(chat.class),
        InferRequest::Embed(embed) => Some(embed.class),
        InferRequest::Task(task) => Some(task.class),
        InferRequest::Speak(speak) => Some(speak.class),
        _ => None,
    }
}

impl<K: KeySource + 'static> InferSession for EngineSession<K> {
    async fn send(&mut self, frame: ClientFrame) -> Result<(), SessionError> {
        match frame.attachments() {
            0 => self.send_with(frame, Frames::default()).await,
            named => Err(SessionError::Malformed(format!(
                "the frame names {named} attachments, none were given"
            ))),
        }
    }

    #[cfg(unix)]
    async fn send_attached(
        &mut self,
        frame: ClientFrame,
        attachments: Vec<std::os::fd::OwnedFd>,
    ) -> Result<(), SessionError> {
        let named = frame.attachments();
        if attachments.len() != named {
            return Err(SessionError::Malformed(format!(
                "the frame names {named} attachments, {} were given",
                attachments.len()
            )));
        }
        let frames = Frames::read(attachments)
            .map_err(|_| SessionError::Malformed("an attachment could not be read".to_owned()))?;
        self.send_with(frame, frames).await
    }

    async fn next(&mut self) -> Result<InferEvent, SessionError> {
        if let Phase::Refused(refusal) = &mut self.phase {
            return refusal
                .take()
                .map(|why| InferEvent::Finished(InferReply::Refused(why)))
                .ok_or(SessionError::Closed);
        }
        // The session's own events first (a `Cancelled` comes before the next turn's events),
        // then the running turn's; only the turn's `Finished` ends the turn.
        let (event, of_turn) = match &mut self.phase {
            Phase::Running(_, turn) => tokio::select! {
                biased;
                event = self.inbox.recv() => (event, false),
                event = turn.inbox.recv() => (event, true),
            },
            _ => (self.inbox.recv().await, false),
        };
        let event = event.ok_or(SessionError::Closed)?;
        if of_turn && matches!(event, InferEvent::Finished(_)) {
            let phase = std::mem::replace(&mut self.phase, Phase::Refused(None));
            self.phase = match phase {
                Phase::Running(pinned, _) => Phase::Idle(pinned),
                other => other,
            };
        }
        Ok(event)
    }
}
