//! Provenance (the companion's shared who and what): the ids every agent-side crate names, the
//! four effects, the merged [`Actor`], the FIDES label lattice that follows data, and the
//! confirmation receipt that is the only witness able to raise a label.
//!
//! It lives in porter because memory (almanac) and the action router (docket) both need these
//! types and neither may depend on the other. Pure and portable: serde only.
//!
//! The lattice behaviour (`Label::join`, endorsement, declassification) is frozen as signatures
//! with `todo!()` bodies; `FINDINGS.md` lists each.

mod actor;
mod consent;
mod effect;
mod ids;
mod label;

pub use actor::{Actor, ActorKind, AgentRole, Channel, SystemPart};
pub use consent::{ConfirmId, ConfirmReceipt, InputProof, Witness, declassify, endorse};
pub use effect::Effect;
pub use ids::{ActionName, ClientName, EntityId, EntityKey, EntityKind, RunId, SessionId};
pub use label::{
    Confidentiality, Integrity, Label, Labelled, Measured, ModelRole, Quarantined, ReaderKey,
    Source,
};
/// Re-exported so `prov` is the one import for identity vocabulary.
pub use porter_core::{AppName, DataClass, SpaceId, SpaceScope, UnixSeconds};
