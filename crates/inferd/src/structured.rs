//! Structured replies: validate and repair (ARCHITECTURE section 7, stoker interface ask 100).
//!
//! The check and the repair moved to `porter_turns::structured`, and the limits they run under
//! to `porter_turns::structured::limits`; both stay importable from here. What stays is the
//! `[ai]` table of `inferd.toml` ([`AiConfig`]), whose `[ai.structured]` rows are those limits.

mod ai;
#[cfg(test)]
mod limits_tests;

pub use ai::AiConfig;
pub use porter_turns::structured::{
    Checked, Limits, Resolved, Shaping, StructuredConfig, Validated, run, shaping,
};

/// The limits' rows and types, moved to `porter_turns::structured::limits`; the `[ai]` table
/// that holds them stays here.
pub mod limits {
    pub use super::ai::AiConfig;
    pub use porter_turns::structured::limits::{
        DEPTH, Limits, OPEN_LIST, OPEN_TEXT, REPAIR_BUDGET, Resolved, Row, StructuredConfig,
    };
}
