//! Does an offer meet a need? The one place a query meets the vocabulary.

use crate::need::Need;
use crate::short::{Shortfall, Wants};
use porter_capability::Capability;
use porter_capability::{AbsentReason, Offer};
use serde::{Deserialize, Serialize};

/// The answer for one need against one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Match {
    /// The offer meets every minimum.
    Fits,
    /// Same kind, but this field falls short (the first one, in the need's field order).
    Short(Shortfall),
    /// Same kind, absent, for this reason.
    Absent(AbsentReason),
    /// A different kind altogether.
    OtherKind,
}

/// How `offer` answers `need`.
pub fn matches(need: &Need, offer: &Offer) -> Match {
    match offer {
        Offer::Absent { kind, reason } if *kind == need.kind() => Match::Absent(*reason),
        Offer::Absent { .. } => Match::OtherKind,
        Offer::Present(capability) => fit(need, capability),
    }
}

macro_rules! define_fit {
    ($($(#[$doc:meta])* $variant:ident($cap:ty, $need:ty, $name:literal $(, $ai:ident)?),)*) => {
        /// `need` against `capability`: the check of the need's own kind, or `OtherKind`.
        fn fit(need: &Need, capability: &Capability) -> Match {
            match (need, capability) {
                $((Need::$variant(n), Capability::$variant(c)) => {
                    n.short_of(c).map_or(Match::Fits, Match::Short)
                })*
                _ => Match::OtherKind,
            }
        }
    };
}

porter_capability::capability_kinds!(define_fit);

#[cfg(test)]
mod tests;
