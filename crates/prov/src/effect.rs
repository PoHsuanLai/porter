//! What an action can do to the world: the four effects of the companion's rule 9.

use serde::{Deserialize, Serialize};

/// The effect class of an action, ordered by severity: `Read < UndoableWrite < Outbound <
/// Destructive`. Policy compares with `>=`; the router asks per class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Looks, changes nothing.
    Read,
    /// Changes something the app can take back (archive, move, label, a draft).
    UndoableWrite,
    /// Sends something to another person or service.
    Outbound,
    /// Removes something for good.
    Destructive,
}

impl Effect {
    /// The effect's slug for a span attribute: the same text as its serde form, as a `&'static
    /// str`. A test pins the two together.
    pub const fn slug(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::UndoableWrite => "undoable_write",
            Effect::Outbound => "outbound",
            Effect::Destructive => "destructive",
        }
    }
}
