//! Provenance (the companion's shared who and what): the ids every agent-side crate names, the
//! four effects, the merged [`Actor`], the FIDES label lattice that follows data, and the
//! confirmation receipt that is the only witness able to raise a label.
//!
//! It lives in porter because memory (almanac) and the action router (docket) both need these
//! types and neither may depend on the other. Pure and portable: serde only.
//!
//! The one message model ([`Message`]) every agent uses, the roster's [`AgentRef`], and the
//! rule for how the `desktop` scope joins a Space's labels ([`Confidentiality::join`]).
//! **A message carries no authority**: it is input; any request in it is evaluated under the
//! receiver's own `TaskPolicy` and the gating pipeline (see [`message`](Message)).
//!
//! The lattice (`Label::join`, endorsement, declassification) is built and property-tested.

mod actor;
mod agent;
mod consent;
mod effect;
mod ids;
mod label;
mod message;
mod scope;
pub mod trace;

pub use actor::{Actor, ActorKind, AgentRole, Channel, SystemPart};
pub use agent::{Address, AgentRef, Crossing};
pub use consent::{
    BeyondReceipt, ConfirmId, ConfirmReceipt, InputProof, Witness, declassify, endorse,
};
pub use effect::Effect;
pub use ids::{
    ActionName, ClientName, EntityId, EntityKey, EntityKind, MessageId, OutcomeRef, RunId,
    SessionId, TaskId, ThreadId, UndoHandle,
};
pub use label::{
    Confidentiality, Integrity, Label, Labelled, Measured, ModelRole, Quarantined, ReaderKey,
    Source,
};
pub use message::{
    Fault, MAX_PARTS, MAX_TEXT_BYTES, Message, MessageKind, MessageText, Part, ReportStatus,
    SenderCheck,
};
/// Re-exported so `prov` is the one import for identity vocabulary.
pub use porter_core::capability::AgentProgram;
pub use porter_core::{AppName, DataClass, SpaceId, SpaceScope, UnixSeconds};
pub use scope::{DesktopVerdict, Flow, desktop_admits};
