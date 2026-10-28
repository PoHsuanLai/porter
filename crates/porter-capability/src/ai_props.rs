//! The properties beside every AI capability (design/31 §2.3): where it runs, which tier the
//! user mapped it to, and what it costs.

use serde::{Deserialize, Serialize};

/// Where a model runs. Ordered by closeness: routing prefers the smallest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Locality {
    /// On this computer.
    OnDevice,
    /// On another machine the user owns (a Tailscale peer); never auto-probed.
    LocalNetwork,
    /// A provider's servers.
    Cloud {
        /// The region, when the account pins one (Azure, Vertex, Bedrock).
        region: Option<Region>,
    },
}

/// A cloud region as the provider names it (`eu-west-1`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Region(pub String);

/// The quality tier an app asks for; the user maps tiers to models per account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Cheapest and quickest.
    Fast,
    /// The everyday default.
    Balanced,
    /// The user's choice for demanding work (a name for a slot, not a ranking of models).
    Best,
}

/// How using a model is paid for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Billing {
    /// Nothing (local runtimes).
    Free,
    /// Per token, charged by the provider. What a reply cost is what the provider reports for it
    /// (porter keeps no price table), and a provider that reports none shows tokens only.
    Metered,
    /// A subscription's allowance: requests count against a rate budget, not money.
    PlanBudget,
}
