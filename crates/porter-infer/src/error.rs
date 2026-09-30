//! Why inferd refuses, and why a model call fails.

use porter_core::DataClass;
use serde::{Deserialize, Serialize};

/// Why a request was not run; each tells the app what to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum InferRefusal {
    /// Only cloud models fit, and this class may not leave the machine: the app says so and
    /// may offer Settings.
    #[error("{0:?} data may not leave this computer")]
    RequiresCloud(DataClass),
    /// No model fits and is reachable (local-only with no local runtime running).
    #[error("no model available")]
    Unavailable,
    /// The app holds no grant for a fitting account; it must ask.
    #[error("needs consent")]
    NeedsGrant,
    /// The user refused this app.
    #[error("denied")]
    Denied,
    /// Every fitting account has reached its spend cap for this app.
    #[error("spend cap reached")]
    OverBudget,
}

/// Why one model call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// The runtime or provider could not be reached.
    #[error("unreachable")]
    Unreachable,
    /// Rate limited; retry after this many seconds.
    #[error("rate limited for {0} s")]
    RateLimited(u32),
    /// The key or token was refused.
    #[error("unauthorized")]
    Unauthorized,
    /// The provider refused the content.
    #[error("refused by the provider")]
    Refused,
    /// The answer could not be read.
    #[error("unreadable answer")]
    Unreadable,
}
