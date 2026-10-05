//! The structured-output limits: what the settings rows `ai.structured.open_text`, `open_list`,
//! `depth` and `repair_budget` (design/22 section 3.26, interface ask 143) say, as the values
//! `structured` runs under.
//!
//! The daemon reads `[ai.structured]` of `inferd.toml` and passes the result down; nothing here
//! reads a file or the environment. A value outside its row's range is not clamped: as every
//! settings file does (design/22 section 2), it falls back to the row's default, for that field
//! only, and is named in [`Resolved::rejected`] so the daemon can log it once.

use model_extract::RepairBudget;
use model_provider::{CharCount, Count, SchemaLimits};
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

/// `ai.structured.open_text`: default and range, in characters.
pub const OPEN_TEXT: Row = Row::new("ai.structured.open_text", 4096, 256..=65536);
/// `ai.structured.open_list`: default and range, in items.
pub const OPEN_LIST: Row = Row::new("ai.structured.open_list", 256, 16..=4096);
/// `ai.structured.depth`: default and range, in levels.
pub const DEPTH: Row = Row::new("ai.structured.depth", 16, 4..=64);
/// `ai.structured.repair_budget`: default and range, in re-asks.
pub const REPAIR_BUDGET: Row = Row::new("ai.structured.repair_budget", 1, 0..=3);

/// One settings row's path, default and range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The dotted settings path.
    pub path: &'static str,
    /// The value when the file has none or has a bad one.
    pub default: u32,
    /// The values the row accepts.
    pub range: RangeInclusive<u32>,
}

impl Row {
    const fn new(path: &'static str, default: u32, range: RangeInclusive<u32>) -> Self {
        Self {
            path,
            default,
            range,
        }
    }

    /// The row's value for what the file said: itself in range, else the default (and the path,
    /// when the file said something out of range).
    fn pick(&self, said: Option<i64>) -> (u32, Option<&'static str>) {
        match said {
            None => (self.default, None),
            Some(value) => match u32::try_from(value) {
                Ok(value) if self.range.contains(&value) => (value, None),
                _ => (self.default, Some(self.path)),
            },
        }
    }
}

/// The `[ai.structured]` table, as written. Signed, so a negative number reaches the range check
/// and is rejected there instead of failing the whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredConfig {
    /// `ai.structured.open_text`.
    #[serde(default)]
    pub open_text: Option<i64>,
    /// `ai.structured.open_list`.
    #[serde(default)]
    pub open_list: Option<i64>,
    /// `ai.structured.depth`.
    #[serde(default)]
    pub depth: Option<i64>,
    /// `ai.structured.repair_budget`.
    #[serde(default)]
    pub repair_budget: Option<i64>,
}

/// The `[ai]` table: the settings rows of the `ai` domain that inferd reads and that are not the
/// policy or the tier map (those keep their own tables).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiConfig {
    /// `ai.structured.*`.
    #[serde(default)]
    pub structured: StructuredConfig,
}

/// What a structured turn runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// What a schema may leave open, and how deep it may nest.
    pub schema: SchemaLimits,
    /// The re-asks a reply that fails its schema gets.
    pub repairs: RepairBudget,
}

impl Default for Limits {
    fn default() -> Self {
        AiConfig::default().resolve().limits
    }
}

/// The limits in force, and the rows whose values were refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The limits.
    pub limits: Limits,
    /// The paths of the rows that held an out-of-range value and fell back to their default.
    pub rejected: Vec<&'static str>,
}

impl AiConfig {
    /// The limits for this configuration.
    pub fn resolve(&self) -> Resolved {
        let said = &self.structured;
        let (open_text, a) = OPEN_TEXT.pick(said.open_text);
        let (open_list, b) = OPEN_LIST.pick(said.open_list);
        let (depth, c) = DEPTH.pick(said.depth);
        let (repairs, d) = REPAIR_BUDGET.pick(said.repair_budget);
        Resolved {
            limits: Limits {
                schema: SchemaLimits {
                    open_text: CharCount(open_text),
                    open_list: Count(open_list),
                    depth: Count(depth),
                },
                // The range ends at 3.
                repairs: RepairBudget(u8::try_from(repairs).unwrap_or(u8::MAX)),
            },
            rejected: [a, b, c, d].into_iter().flatten().collect(),
        }
    }
}
