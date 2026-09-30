//! Routing (design/31 §5.5): drop what the policy forbids, then what the app may not use or
//! afford, then rank this computer first, the user's tier choice next, cost last. Every way to
//! come up empty is a typed refusal, never a silent downgrade to the cloud.

use crate::error::InferRefusal;
use crate::policy::{LocalOnly, Policy};
use crate::spend::SpendVerdict;
use porter_core::consent::Verdict;
use porter_core::{AccountId, Billing, DataClass, Locality, ModelId};

/// What routing needs to know about the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteAsk {
    /// The class of data it carries.
    pub class: DataClass,
}

/// Whether the user mapped this model to the tier the request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TierChoice {
    /// It is the user's choice for that tier.
    Chosen,
    /// It fits, but another model is the tier's choice.
    Other,
}

/// One model whose capability meets the request, with what the broker knows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCandidate {
    /// The account.
    pub account: AccountId,
    /// The model.
    pub model: ModelId,
    /// Where it runs.
    pub locality: Locality,
    /// What it costs.
    pub billing: Billing,
    /// Whether it is the tier's choice.
    pub tier: TierChoice,
    /// The app's consent for this account, kind and class.
    pub permission: Verdict,
    /// The spend caps' verdict for this request.
    pub spend: SpendVerdict,
}

/// The model to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    /// The account.
    pub account: AccountId,
    /// The model.
    pub model: ModelId,
    /// `Warn` when the request passes a cap's warning line.
    pub spend: SpendVerdict,
}

/// The model to run for `ask`, or why none may.
pub fn route(
    ask: RouteAsk,
    candidates: &[RouteCandidate],
    policy: &Policy,
) -> Result<Chosen, InferRefusal> {
    let reachable: Vec<&RouteCandidate> = candidates
        .iter()
        .filter(|c| policy.local_only == LocalOnly::Off || !is_cloud(&c.locality))
        .collect();
    if reachable.is_empty() {
        return Err(InferRefusal::Unavailable);
    }
    let floor = policy.floor(ask.class);
    let admitted: Vec<&RouteCandidate> = reachable
        .into_iter()
        .filter(|c| floor.admits(&c.locality))
        .collect();
    if admitted.is_empty() {
        return Err(InferRefusal::RequiresCloud(ask.class));
    }
    let permitted: Vec<&RouteCandidate> = admitted
        .iter()
        .copied()
        .filter(|c| matches!(c.permission, Verdict::Granted { .. }))
        .collect();
    if permitted.is_empty() {
        let askable = admitted.iter().any(|c| c.permission == Verdict::Ask);
        return Err(if askable {
            InferRefusal::NeedsGrant
        } else {
            InferRefusal::Denied
        });
    }
    permitted
        .into_iter()
        .filter(|c| c.spend != SpendVerdict::Stop)
        .min_by_key(|c| (closeness(&c.locality), c.tier, price_rank(&c.billing)))
        .map(|c| Chosen {
            account: c.account.clone(),
            model: c.model.clone(),
            spend: c.spend,
        })
        .ok_or(InferRefusal::OverBudget)
}

fn is_cloud(locality: &Locality) -> bool {
    matches!(locality, Locality::Cloud { .. })
}

/// This computer, then the user's network, then the cloud.
fn closeness(locality: &Locality) -> u8 {
    match locality {
        Locality::OnDevice => 0,
        Locality::LocalNetwork => 1,
        Locality::Cloud { .. } => 2,
    }
}

/// Free, then plan allowance, then metered by list price.
fn price_rank(billing: &Billing) -> (u8, u64) {
    match billing {
        Billing::Free => (0, 0),
        Billing::PlanBudget => (1, 0),
        Billing::Metered(price) => (2, price.input_per_mtok.0 + price.output_per_mtok.0),
    }
}

#[cfg(test)]
mod tests;
