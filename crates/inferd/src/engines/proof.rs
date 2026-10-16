//! The tool-call check on a model of this computer: the canned request of
//! `porter_infer::probe_request`, run the first time Automatic considers the model for a request
//! that offers tools, and the result kept ([`crate::toolproof::ToolProofs`]).
//!
//! A hosted model is not checked (see the `toolproof` module). A model that could not be started,
//! or whose turn ended for a reason that says nothing about tool calls (unreachable, rate
//! limited), is left unknown and checked again at the next session, never marked failed.

use super::{Engines, Offered, through_grants};
use crate::runner::{Pin, Turns};
use crate::serve::{EngineHost, RunningTurn, TurnRunner, TurnStep};
use crate::session::SessionSpec;
use crate::toolproof::asks_for_tools;
use porter_core::Tier;
use porter_infer::{
    InferReply, InferRequest, ModelError, ModelRef, Pick, Slot, ToolProof, judge, probe_request,
};
use std::time::Duration;

/// The longest the check waits on one model, loading included.
const PROBE_WAIT: Duration = Duration::from_secs(180);

impl Engines {
    /// Checks the models of Automatic's pool that nothing is known of yet, when `spec` offers
    /// tools and its text slot is Automatic. Nothing for any other request.
    pub(crate) async fn prove_tools(&self, spec: &SessionSpec, offered: &Offered) {
        if !asks_for_tools(&spec.need) {
            return;
        }
        let tiers = through_grants(&self.settings().tiers, offered);
        let Some(Pick::Auto { pool, .. }) = tiers.pick_serving(Slot::Text, spec.tier) else {
            return;
        };
        for model in pool {
            if self.book.proofs.get(&model).is_none()
                && let Some(proof) = self.probe_tools(&model, offered).await
            {
                self.book.proofs.record(model, proof);
            }
        }
    }

    /// What the canned request shows of `model`; none for a hosted model and for one that could
    /// not answer at all.
    async fn probe_tools(&self, model: &ModelRef, offered: &Offered) -> Option<ToolProof> {
        if self.local(model).is_none() {
            return None;
        }
        let request = probe_request()?;
        self.want(model.clone()).await.ok()?;
        let pinned = self.pinned_for(model, offered)?;
        let turns =
            Turns::new(Pin::new(), self.supervised.clone(), Tier::Balanced).pinned_to(pinned);
        let mut turn = turns.start(InferRequest::Chat(request), Vec::new());
        let reply = tokio::time::timeout(PROBE_WAIT, async {
            loop {
                if let TurnStep::Done(reply) = turn.next().await {
                    return reply;
                }
            }
        })
        .await
        .ok()?;
        match reply {
            InferReply::Chat(chat) => Some(judge(&chat)),
            InferReply::Failed(ModelError::Unparseable) => Some(ToolProof::Failed),
            _ => None,
        }
    }
}
