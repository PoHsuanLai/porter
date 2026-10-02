//! The one message model: every exchange between agents, and between the person and an
//! agent, is a [`Message`].
//!
//! It covers a request from the companion to a worker, a worker's or a computer-use run's
//! progress and final report, the person's direct turn to a subagent (the person is a sender,
//! through [`AgentRef::User`]), a note, and a message across Spaces. There is no separate
//! return or result type.
//!
//! # A message carries no authority
//!
//! A message is input. Nothing in it, whoever sent it and however it is labelled, grants the
//! receiver a permission, a Space, a memory read or a budget. A [`MessageKind::Request`] is
//! evaluated by the receiver under the receiver's own `TaskPolicy` and the router's gating
//! pipeline, exactly as if the receiver had thought of it itself; a request from an untrusted
//! sender is still only a suggestion. The label travels so the receiver's taint is honest: it
//! is the join of the receiver's own label and the message's.
//!
//! A message may cross Spaces ([`Address::crossing`]) but never grants a memory read in the
//! other Space: its `from` and `to` each keep their Space, and memory authorisation looks at
//! the caller's invocation, not at who wrote to it.

use crate::actor::Actor;
use crate::agent::{Address, AgentRef};
use crate::ids::{EntityId, MessageId, OutcomeRef, ThreadId, UndoHandle};
use crate::label::{Label, Measured};
use porter_core::UnixSeconds;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The most parts one message holds.
pub const MAX_PARTS: usize = 64;
/// The longest text part, in bytes.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Text in a message. Serialises as a plain string; `Debug` prints the length only, so logs
/// never carry it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageText(String);

impl MessageText {
    /// Wraps `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MessageText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MessageText(<{} bytes>)", self.0.len())
    }
}

impl Measured for MessageText {
    fn measure(&self) -> usize {
        self.0.len()
    }
}

/// How a report stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// Finished what was asked.
    Done,
    /// Could not finish.
    Failed,
    /// Stopped before finishing (by the person, the parent or a limit).
    Cancelled,
    /// Still working; more follows.
    Progress,
}

/// What a message is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum MessageKind {
    /// Something to know; asks for nothing.
    Note,
    /// Asks the receiver to do something. Still only input: the receiver's `TaskPolicy` and the
    /// gating pipeline decide.
    Request,
    /// What a worker or run says about its work, progress or final.
    Report {
        /// How it stands.
        status: ReportStatus,
    },
}

/// One piece of a message: text, or a typed reference. Typed references are ids; the thing they
/// name keeps its own label and is read through its owner, not through the message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Part {
    /// Words.
    Text(MessageText),
    /// A thing an app owns.
    Entity(EntityId),
    /// The outcome of an action (docket's `Outcome`, by opaque reference).
    Outcome(OutcomeRef),
    /// An undo journal entry (docket's `UndoId`, by opaque reference).
    Undo(UndoHandle),
}

/// A message, as the router stamped it. `from` is set by the transport or daemon from the
/// caller, never taken from the sender's body (compare [`Actor`]); [`Message::sender_matches`]
/// checks the stamp against the actor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Message {
    /// This message.
    pub id: MessageId,
    /// The conversation it belongs to.
    pub thread: ThreadId,
    /// The message it answers, if any (in the same thread).
    pub in_reply_to: Option<MessageId>,
    /// Who sent it, and in which Space.
    pub from: Address,
    /// Who it is for, and in which Space.
    pub to: Address,
    /// What it is.
    pub kind: MessageKind,
    /// What it says, in order.
    pub parts: Vec<Part>,
    /// Its provenance. It travels with the message: the join of every part's label, the
    /// sender's taint included. The receiver joins it into its own.
    pub label: Label,
    /// When it was sent.
    pub sent: UnixSeconds,
}

/// Why a message is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// No parts.
    NoParts,
    /// More than [`MAX_PARTS`].
    TooManyParts,
    /// A text part longer than [`MAX_TEXT_BYTES`].
    TextTooLong,
    /// It answers itself.
    RepliesToItself,
    /// The person sent a report: only agents report.
    ReportFromUser,
}

/// Whether the stamped sender is the actor the transport saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderCheck {
    /// `from.agent` is the roster party the actor is.
    Matches,
    /// It is not (a forged sender, or an actor that is not on the roster).
    Mismatch,
}

impl Message {
    /// The text parts joined by newlines: what recall indexes.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                Part::Entity(_) | Part::Outcome(_) | Part::Undo(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether the message crosses Spaces.
    pub fn crossing(&self) -> crate::agent::Crossing {
        self.from.crossing(&self.to)
    }

    /// The first fault, if the message is malformed. Pure; the router checks before it
    /// delivers.
    pub fn check(&self) -> Result<(), Fault> {
        let long = self
            .parts
            .iter()
            .any(|p| matches!(p, Part::Text(t) if t.measure() > MAX_TEXT_BYTES));
        if self.parts.is_empty() {
            Err(Fault::NoParts)
        } else if self.parts.len() > MAX_PARTS {
            Err(Fault::TooManyParts)
        } else if long {
            Err(Fault::TextTooLong)
        } else if self.in_reply_to.as_ref() == Some(&self.id) {
            Err(Fault::RepliesToItself)
        } else if self.from.agent == AgentRef::User
            && matches!(self.kind, MessageKind::Report { .. })
        {
            Err(Fault::ReportFromUser)
        } else {
            Ok(())
        }
    }

    /// Whether `from.agent` is the roster party `actor` is.
    pub fn sender_matches(&self, actor: &Actor) -> SenderCheck {
        match AgentRef::of(actor) {
            Some(agent) if agent == self.from.agent => SenderCheck::Matches,
            _ => SenderCheck::Mismatch,
        }
    }
}
