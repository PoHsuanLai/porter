//! Converting porter's values to and from their D-Bus shapes. Parse at the boundary: a daemon
//! turns a `NeedArg` into a `Need` here and nowhere else. Frozen signatures; the bodies are
//! not built yet.

use crate::args::{CandidateArg, NeedArg};
use porter_core::{Candidate, CoreError, Need};

/// A need as its D-Bus argument.
pub fn need_to_dbus(_need: &Need) -> NeedArg {
    todo!("the kind's serde slug, then each field's slug under its field name")
}

/// The need a D-Bus argument carries, or why it is not one.
pub fn need_from_dbus(_arg: NeedArg) -> Result<Need, CoreError> {
    todo!("parse the kind slug, then each field from the vardict")
}

/// A candidate as its D-Bus shape.
pub fn candidate_to_dbus(_candidate: &Candidate) -> CandidateArg {
    todo!("account_path, label, then provider, capability, restriction and grant by name")
}

/// The candidate a D-Bus value carries, or why it is not one.
pub fn candidate_from_dbus(_arg: CandidateArg) -> Result<Candidate, CoreError> {
    todo!("the inverse of candidate_to_dbus")
}
