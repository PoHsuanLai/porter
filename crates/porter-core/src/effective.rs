//! The effective capabilities of an account (design/31 §2.4): per subject and kind, the most
//! authoritative claim wins, then the user's toggles turn kinds off.

use crate::capability::CapabilityKind;
use crate::offer::{AbsentReason, Claim, Offer};
use serde::{Deserialize, Serialize};

/// Whether the user lets an account use one of its kinds (Settings' per-account toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Toggle {
    /// In use.
    On,
    /// Turned off by the user.
    Off,
}

/// A user's toggle for one kind of one account. Kinds without one are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KindToggle {
    /// The kind.
    pub kind: CapabilityKind,
    /// On or off.
    pub toggle: Toggle,
}

/// The claims that hold: one per (subject, kind), the highest [`crate::Provenance`] winning
/// (the first of equals), in the order each (subject, kind) first appears; kinds toggled off
/// become `Absent { TurnedOff }` at the winner's provenance.
pub fn effective(claims: &[Claim], toggles: &[KindToggle]) -> Vec<Claim> {
    let winners = claims.iter().fold(Vec::<Claim>::new(), |mut held, claim| {
        let same = |c: &Claim| c.subject == claim.subject && c.offer.kind() == claim.offer.kind();
        match held.iter_mut().find(|c| same(c)) {
            Some(slot) if claim.provenance > slot.provenance => *slot = claim.clone(),
            Some(_) => {}
            None => held.push(claim.clone()),
        }
        held
    });
    winners
        .into_iter()
        .map(|claim| apply_toggle(claim, toggles))
        .collect()
}

fn apply_toggle(claim: Claim, toggles: &[KindToggle]) -> Claim {
    let kind = claim.offer.kind();
    let off = toggles
        .iter()
        .any(|t| t.kind == kind && t.toggle == Toggle::Off);
    if off {
        Claim {
            offer: Offer::Absent {
                kind,
                reason: AbsentReason::TurnedOff,
            },
            ..claim
        }
    } else {
        claim
    }
}

#[cfg(test)]
mod tests;
