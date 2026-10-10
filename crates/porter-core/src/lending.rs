//! The plain words and rows of one computer lending its models to the person's others, as
//! `org.quire.Inference1` hands them to Settings and the shell: who is asking, what the person
//! answered, and which computers could be added. Pure data, no files and no sockets: the one
//! program that keeps the answers is `porter-tailnet`, and the client reads them through
//! `porter-client`. They live here so a client that only talks to the bus builds none of the
//! lending computer's own dependencies.

use crate::tailnet::NodeId;
use crate::units::UnixSeconds;

/// The person's answer to a computer: what `AnswerGuest` takes, as a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuestAnswer {
    /// Let it use this computer's models.
    Allow,
    /// Keep it out.
    Deny,
}

impl GuestAnswer {
    /// Both answers, for tables.
    pub const ALL: [GuestAnswer; 2] = [GuestAnswer::Allow, GuestAnswer::Deny];

    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            GuestAnswer::Allow => "allow",
            GuestAnswer::Deny => "deny",
        }
    }

    /// The answer a word names; none for any other word.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|answer| answer.slug() == slug)
    }
}

/// How one computer stands, as the list shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    /// The person said yes.
    Approved,
    /// The person said no.
    Denied,
    /// It is waiting for an answer.
    Asking,
}

impl RowState {
    /// Every state, for tables.
    pub const ALL: [RowState; 3] = [RowState::Approved, RowState::Denied, RowState::Asking];

    /// The word on the bus.
    pub fn slug(self) -> &'static str {
        match self {
            RowState::Approved => "approved",
            RowState::Denied => "denied",
            RowState::Asking => "asking",
        }
    }

    /// The state a word names; none for any other word.
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.slug() == slug)
    }
}

/// One line of the list of computers that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GuestRow {
    /// The computer's stable id.
    pub node: NodeId,
    /// Its name.
    pub name: String,
    /// How it stands.
    pub state: RowState,
    /// When it was answered, or began asking.
    pub since: UnixSeconds,
}

impl GuestRow {
    /// A line for the computer `node`, called `name`, standing as `state` since `since`.
    pub fn new(node: NodeId, name: String, state: RowState, since: UnixSeconds) -> Self {
        Self {
            node,
            name,
            state,
            since,
        }
    }
}

/// Where the asking computer stands with the person on the lending one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// The person has yet to say yes to it.
    Needed,
    /// The person said yes.
    Given,
}

/// What the `GuestAsks` signal says of a question: the computer's name and when it began
/// asking. The computer's id travels beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GuestAsk {
    /// Its name now.
    pub name: String,
    /// When it began asking.
    pub since: UnixSeconds,
}

impl GuestAsk {
    /// A question from the computer called `name`.
    pub fn new(name: String, since: UnixSeconds) -> Self {
        Self { name, since }
    }
}

/// One model a computer lends: its id in the catalogue both computers share, and the name shown.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CandidateModel {
    /// The catalogue's id for the model.
    pub id: String,
    /// The name shown to the person.
    pub name: String,
}

impl CandidateModel {
    /// A model with its catalogue id and shown name.
    pub fn new(id: String, name: String) -> Self {
        Self { id, name }
    }
}

/// A computer of the person's that lends its models and is not added yet, as `Candidates` lists
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComputerCandidate {
    /// Its stable id: what `add_tailnet_computer` takes.
    pub node: NodeId,
    /// The name people call it by.
    pub name: String,
    /// The models it lends.
    pub models: Vec<CandidateModel>,
    /// Whether the person still has to say yes on that computer.
    pub approval: Approval,
}

impl ComputerCandidate {
    /// A candidate computer.
    pub fn new(
        node: NodeId,
        name: String,
        models: Vec<CandidateModel>,
        approval: Approval,
    ) -> Self {
        Self {
            node,
            name,
            models,
            approval,
        }
    }
}
