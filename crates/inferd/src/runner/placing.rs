//! Where a request of a local model runs and what becomes of it when it fails: which engine
//! instance of the model, after waiting for a seat (the person's turn first); a plain sentence
//! for a request the engine could not finish; and one more try, on another running instance, for
//! the person's own request that lost its engine before it had said anything.

use super::{Job, Seating, refused};
use crate::admit::{Priority, Seats};
use crate::bridge::{self, Frames};
use crate::failure::{EngineView, Retry, Spoke, Tried, explain, log_line, retry};
use crate::local::LocalModel;
use crate::seats::{Pass, Standing, seats_in};
use crate::serve::TurnStep;
use crate::structured::{self, Shaping};
use crate::supervise::Supervised;
use engine_supervisor::EngineId;
use model_catalog::EngineKind;
use porter_infer::{
    InferEvent, InferRefusal, InferReply, InferRequest, ModelError, QueueFact, ServedBy,
};
use porter_turns::local::LocalTurns;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Whether `model` is an engine this daemon launches to chat, which is what has seats. A runtime
/// the person runs themselves, a replay and the speech engines take every request at once.
fn has_seats(model: &LocalModel) -> bool {
    model.attached.is_none()
        && model.loopback.is_none()
        && model.cassette.is_none()
        && matches!(
            model.profile.kind,
            EngineKind::Vllm | EngineKind::LlamaServer
        )
}

/// The error of a reply that failed, if it did.
fn failed(reply: &InferReply) -> Option<ModelError> {
    match reply {
        InferReply::Failed(error) => Some(*error),
        _ => None,
    }
}

impl Job {
    /// Tells the session where it stands in the queue.
    fn tell(&self, fact: QueueFact) {
        let _ = self.steps.send(TurnStep::Event(InferEvent::Queue(fact)));
    }

    /// The instance a request runs on and its seat on it. The request waits for the seat,
    /// saying where it stands; a request that waited on an instance that has since stopped, when
    /// another instance of its model is running, gives up its place and goes there. Without
    /// seating (no gate) or for a model with no seats, the request runs at once on `pinned`.
    pub(super) async fn place(
        &self,
        pinned: &Arc<LocalModel>,
        avoid: Option<&EngineId>,
    ) -> (Arc<LocalModel>, Option<Pass>) {
        let Some(seating) = self.seating.as_ref().filter(|_| has_seats(pinned)) else {
            return (Arc::clone(pinned), None);
        };
        let (model, pass) = self.seat_among(seating, pinned, avoid).await;
        let stopped = seating.engines.view_of(&model) == EngineView::NotRunning;
        let elsewhere = !seating
            .engines
            .running_instances(&pinned.model_ref(), avoid)
            .is_empty();
        if stopped && elsewhere {
            drop(pass);
            let (model, pass) = self.seat_among(seating, pinned, avoid).await;
            return (model, Some(pass));
        }
        (model, Some(pass))
    }

    /// Takes a seat on a running instance of `pinned`'s model (on `pinned` itself when none is
    /// running: it waits for the supervisor to start it), waiting its turn.
    async fn seat_among(
        &self,
        seating: &Seating,
        pinned: &Arc<LocalModel>,
        avoid: Option<&EngineId>,
    ) -> (Arc<LocalModel>, Pass) {
        let mut candidates = seating
            .engines
            .running_instances(&pinned.model_ref(), avoid);
        if candidates.is_empty() {
            candidates = vec![Arc::clone(pinned)];
        }
        let instances: Vec<(EngineId, Seats)> = candidates
            .iter()
            .map(|one| {
                (
                    one.spec.id.clone(),
                    seats_in(one.profile.kind, &one.profile.args),
                )
            })
            .collect();
        let mut pass = seating.gate.pass_among(&instances, &seating.asker);
        let mut told = false;
        while let Standing::Waiting { ahead } = pass.standing() {
            told = true;
            self.tell(QueueFact::Waiting { ahead });
            if !pass.moved().await {
                break;
            }
        }
        if told {
            self.tell(QueueFact::Started);
        }
        let chosen = pass
            .engine()
            .and_then(|engine| candidates.iter().find(|one| one.spec.id == *engine))
            .map_or_else(|| Arc::clone(pinned), Arc::clone);
        (chosen, pass)
    }

    /// A chat, task or embedding request: seated, run, told in plain words if it failed, and run
    /// once more elsewhere when the person's own request lost its engine before it had said
    /// anything.
    pub(super) async fn retrying(
        &self,
        pinned: &Arc<LocalModel>,
        served: &ServedBy,
        request: &InferRequest,
        frames: &Frames,
    ) -> InferReply {
        let who = self
            .seating
            .as_ref()
            .map_or(Priority::Helper, |seating| Priority::from(&seating.asker));
        let mut avoid: Option<EngineId> = None;
        let mut tried = Tried::Never;
        loop {
            let (model, seat) = self.place(pinned, avoid.as_ref()).await;
            self.engines.used(&model.spec.id);
            let (reply, spoke) = self.tapped(&model, served, request, frames).await;
            let reply = self.explained(&model, reply).await;
            drop(seat);
            let Some(error) = failed(&reply) else {
                return reply;
            };
            let others = self.seating.as_ref().map_or(0, |seating| {
                seating
                    .engines
                    .running_instances(&pinned.model_ref(), Some(&model.spec.id))
                    .len()
            });
            match retry(who, error, spoke, tried, others) {
                Retry::Elsewhere => {
                    avoid = Some(model.spec.id.clone());
                    tried = Tried::Once;
                }
                Retry::No => return reply,
            }
        }
    }

    /// One try of a request on `model`, noting whether it sent the session anything.
    async fn tapped(
        &self,
        model: &LocalModel,
        served: &ServedBy,
        request: &InferRequest,
        frames: &Frames,
    ) -> (InferReply, Spoke) {
        let (tap, mut seen) = mpsc::unbounded_channel::<TurnStep>();
        let out = self.steps.clone();
        let forward = tokio::spawn(async move {
            let mut spoke = Spoke::Nothing;
            while let Some(step) = seen.recv().await {
                spoke = Spoke::Something;
                let _ = out.send(step);
            }
            spoke
        });
        let local = LocalTurns::new(self.engines.clone(), tap, Arc::clone(&self.run));
        let reply = self.borrowed(&local, model, served, request, frames).await;
        drop(local);
        (reply, forward.await.unwrap_or(Spoke::Something))
    }

    /// A chat, task or embedding request on `model` through `local`.
    async fn borrowed(
        &self,
        local: &LocalTurns<Supervised>,
        model: &LocalModel,
        served: &ServedBy,
        request: &InferRequest,
        frames: &Frames,
    ) -> InferReply {
        match request {
            InferRequest::Chat(chat) => match bridge::chat_turn(model, chat, frames) {
                Ok(turn) => {
                    let shaping = structured::shaping(model, chat, self.limits);
                    local.chat(model, served, &turn, shaping).await
                }
                Err(_) => refused(InferRefusal::Unsupported),
            },
            InferRequest::Task(task) => match bridge::task_turn(model, task, self.tier) {
                Ok(turn) => local.chat(model, served, &turn, Shaping::Unchecked).await,
                Err(_) => refused(InferRefusal::Unsupported),
            },
            InferRequest::Embed(embed) => local.embed(model, served, embed).await,
            _ => refused(InferRefusal::Unsupported),
        }
    }

    /// A failed reply told in plain words (the engine stopped, the card ran out of memory), and
    /// logged once with the hardware numbers of the moment. Any other reply as it is.
    pub(super) async fn explained(&self, model: &LocalModel, reply: InferReply) -> InferReply {
        let (Some(seating), Some(error)) = (self.seating.as_ref(), failed(&reply)) else {
            return reply;
        };
        if matches!(error, ModelError::Unreachable) {
            seating.engines.noticed(model).await;
        }
        let (moment, facts) = seating.engines.moment_of(model, &seating.gate);
        eprintln!(
            "{}",
            log_line("a reply failed", Some(&model.spec.id), &facts)
        );
        InferReply::Failed(explain(error, &moment))
    }
}
