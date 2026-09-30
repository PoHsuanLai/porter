//! The broker inferd runs: routes each request over its models by the policy, meters spend and
//! writes the audit. Frozen shape; the body is not built yet.

use crate::model::Model;
use crate::policy::Policy;
use crate::reply::InferReply;
use crate::request::InferRequest;
use crate::spend::SpendCap;
use porter_core::AppId;

/// The AI broker over a set of models.
#[derive(Debug)]
pub struct Broker<M: Model> {
    policy: Policy,
    caps: Vec<SpendCap>,
    models: Vec<M>,
}

impl<M: Model> Broker<M> {
    /// A broker with this policy, these caps and these models.
    pub fn new(policy: Policy, caps: Vec<SpendCap>, models: Vec<M>) -> Self {
        Self {
            policy,
            caps,
            models,
        }
    }

    /// The policy in force.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Runs one request for `app`: `route` over the models' cards, the app's grants and spend,
    /// then the chosen model, then the audit entry.
    pub async fn infer(&self, _app: &AppId, _request: InferRequest) -> InferReply {
        let _ = (&self.caps, &self.models);
        todo!("route, run the chosen model, meter spend and audit")
    }
}
