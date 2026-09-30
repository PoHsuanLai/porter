//! The properties beside every AI capability (design/31 §2.3): where it runs, which tier the
//! user mapped it to, and what it costs.

use crate::units::MicroUsd;
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
    /// The strongest model.
    Best,
}

/// How using a model is paid for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Billing {
    /// Nothing (local runtimes).
    Free,
    /// Per token, at these prices.
    Metered(PriceTable),
    /// A subscription's allowance: requests count against a rate budget, not money.
    PlanBudget,
}

/// A model's prices per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PriceTable {
    /// Per million input tokens.
    pub input_per_mtok: MicroUsd,
    /// Per million output tokens.
    pub output_per_mtok: MicroUsd,
}
