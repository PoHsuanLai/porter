//! Computer use (design/31 §2.3): a model that operates a window from screenshots. The dialect
//! detail (grid size, prompt format) stays in stoker's catalog; this is what an app or the
//! companion may ask for.

use super::ai::LlmWire;
use super::terms::Offered;
use crate::units::Px;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A computer-use model.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CuaCap {
    /// The environments it can operate.
    pub environments: BTreeSet<CuaEnv>,
    /// Whether it proposes one action per step or several.
    pub batching: CuaBatching,
    /// Whether it can ask to zoom into a region.
    pub zoom: Offered,
    /// The longest side of a screenshot it takes.
    pub max_image: Px,
    /// The wire format inferd's adapter speaks to it.
    pub wire: LlmWire,
}

/// What a computer-use model operates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuaEnv {
    /// A desktop window.
    Desktop,
    /// A web page.
    Browser,
    /// A phone screen.
    Mobile,
}

/// How many actions one model step may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuaBatching {
    /// One action per step.
    One,
    /// A short batch per step.
    Many,
}
