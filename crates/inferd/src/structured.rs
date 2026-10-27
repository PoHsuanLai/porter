//! Structured replies: validate and repair (ARCHITECTURE section 7, stoker interface ask 100).
//!
//! The check and the repair are `porter_turns::structured`'s, and so are the limits they run
//! under (`porter_turns::structured::limits`). What is here is the `[ai]` table of `inferd.toml` ([`AiConfig`]), whose `[ai.structured]` rows are those limits.

mod ai;
#[cfg(test)]
mod limits_tests;

pub use ai::AiConfig;
