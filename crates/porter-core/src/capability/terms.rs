//! The small enums every data kind shares (design/31 §2.1). Their declaration order is their
//! strength order: a need names a minimum and an offer satisfies it when it is at least that.

use serde::{Deserialize, Serialize};

/// How much of the data an account can touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Nothing (the kind is present only for its other fields, such as upload).
    None,
    /// Reading only.
    Read,
    /// Reading and writing.
    ReadWrite,
}

/// How changes become known (R11): a need asks for a minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    /// Only a full listing shows what changed.
    None,
    /// A change token the client polls (Drive page token, Graph delta, sync-token).
    Poll,
    /// The server pushes (IMAP IDLE, JMAP EventSource, long-poll).
    Push,
}

/// Whether one optional feature is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Offered {
    /// It is not.
    Absent,
    /// It is.
    Present,
}

/// Whether the account reports its storage quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaReport {
    /// No quota figure is available.
    Unreported,
    /// Used and total are reported.
    Reported,
}
