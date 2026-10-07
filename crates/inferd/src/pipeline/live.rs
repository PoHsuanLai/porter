//! A voice chat on a live session: the plan for it and the turn that runs it.
//!
//! The session machine collects the audio (`Phase::Hearing`) and hands it over with the chat
//! ([`crate::serve::TurnRunner::start_heard`]). The turn then plans the request as `plan_pipeline`
//! does for audio in and text out (the text model hears it itself, or `voice_in` does first),
//! over the models the app is offered now, and runs the plan with [`run_pipeline`]: `Ears` on
//! the speech host, then the answering model on the transcript. The stages announce themselves
//! (`Routed` and a `Stage` note each), so the session's own `Answer` note is not sent for the
//! turn: exactly one per turn.

use super::{PipelineInput, VecAudio, plan, run_pipeline};
use crate::engines::{Engines, Offered, through_grants};
use crate::runner::{Turn, Turns};
use crate::serve::{RunningTurn, TurnRunner, TurnStep};
use crate::session::{HeardAudio, SessionSpec};
use crate::speech::Ears;
use porter_core::AppId;
use porter_infer::{
    Answer, ChatRequest, ChatSink, Flow, InferEvent, InferReply, InferRequest, Modality,
    ModelError, ModelRef, RequestShape, ShowReason, StageRole,
};
use std::collections::BTreeSet;
use tokio::sync::mpsc;

/// What a session needs to plan and run a voice chat: the engines, the app whose grants decide
/// the hosted models in reach, and what the session was opened for.
#[derive(Debug, Clone)]
pub struct Hearing {
    /// The daemon's engines (the models, their readiness, the settings).
    pub engines: Engines,
    /// The caller; none offers no hosted model.
    pub app: Option<AppId>,
    /// What the session was opened for.
    pub spec: SessionSpec,
}

impl Hearing {
    /// Plans and runs one voice chat, its events going to `sink`.
    pub(crate) async fn run(
        &self,
        turns: &Turns,
        chat: ChatRequest,
        heard: HeardAudio,
        sink: &mut impl ChatSink,
    ) -> InferReply {
        let spec = &self.spec;
        let offered = match &self.app {
            Some(app) => self.engines.offer(app, spec.class, spec.usage).await,
            None => {
                self.engines.look_at_attached().await;
                Offered::default()
            }
        };
        let settings = self.engines.settings();
        let shape = RequestShape {
            class: spec.class,
            inputs: BTreeSet::from([Modality::Text, Modality::Audio]),
            answer: Answer::Text,
        };
        let planned = match plan(
            &shape,
            &spec.need,
            spec.tier,
            &self.engines.listed_with(&offered),
            &settings.routing_policy(),
            &through_grants(&settings.tiers, &offered),
            settings.auto,
            settings.describe_images,
        ) {
            Ok(planned) => planned,
            Err(refusal) => {
                if let Some(declined) = refusal.declined()
                    && sink.event(InferEvent::Declined(declined.clone())) == Flow::Stop
                {
                    return InferReply::Cancelled;
                }
                return InferReply::Refused(refusal.infer_refusal());
            }
        };
        let model_of = |role| {
            planned
                .stages
                .iter()
                .find(|stage| stage.role == role)
                .map(|stage| ModelRef {
                    account: stage.picked.chosen.account.clone(),
                    model: stage.picked.chosen.model.clone(),
                })
        };
        let Some(answering) = model_of(StageRole::Answer) else {
            return InferReply::Refused(porter_infer::InferRefusal::Unavailable);
        };
        let Some(pinned) = self.engines.pinned_for(&answering, &offered) else {
            return InferReply::Refused(porter_infer::InferRefusal::Unavailable);
        };
        let names = planned
            .stages
            .iter()
            .filter_map(|stage| {
                let model = ModelRef {
                    account: stage.picked.chosen.account.clone(),
                    model: stage.picked.chosen.model.clone(),
                };
                let name = self.engines.label_of(&model, &offered)?;
                Some((model, name))
            })
            .collect();
        let answer = Answering {
            turns: turns.pinned_to(pinned),
            engines: self.engines.clone(),
            model: answering,
        };
        let input = PipelineInput {
            audio: Some((heard.begin, VecAudio(heard.frames.into()))),
            chat,
            names,
        };
        let show: ShowReason = settings.auto.show_reason;
        run_pipeline(
            &planned,
            show,
            input,
            &Ears::new(self.engines.clone()),
            &answer,
            sink,
        )
        .await
    }
}

/// The answering stage's turns: the model is asked for (started if it must be) before its turn
/// runs, since it may not be the one the session's route woke.
struct Answering {
    turns: Turns,
    engines: Engines,
    model: ModelRef,
}

impl TurnRunner for Answering {
    type Turn = Turn;

    fn start(&self, request: InferRequest, attachments: Vec<std::os::fd::OwnedFd>) -> Turn {
        let (steps, inbox) = mpsc::unbounded_channel();
        let (turns, engines, model) =
            (self.turns.clone(), self.engines.clone(), self.model.clone());
        let task = tokio::spawn(async move {
            use crate::serve::EngineHost;
            if engines.want(model).await.is_err() {
                let _ = steps.send(TurnStep::Done(InferReply::Failed(ModelError::NotReady)));
                return;
            }
            let mut inner = turns.start(request, attachments);
            loop {
                let step = inner.next().await;
                let last = matches!(step, TurnStep::Done(_));
                if steps.send(step).is_err() || last {
                    return;
                }
            }
        });
        Turn::running(inbox, task)
    }

    fn start_heard(&self, _: ChatRequest, _: HeardAudio) -> Turn {
        let (steps, inbox) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let _ = steps.send(TurnStep::Done(InferReply::Refused(
                porter_infer::InferRefusal::Unsupported,
            )));
        });
        Turn::running(inbox, task)
    }
}
