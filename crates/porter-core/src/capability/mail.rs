//! Identity and Mail (design/31 §2.2).

use super::terms::{Access, Delta, Offered};
use serde::{Deserialize, Serialize};

/// Who the account is. Every account has at least its label; these say what else it offers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdentityCap {
    /// A display name and an avatar.
    pub profile: Offered,
    /// An address the provider has verified.
    pub verified_address: Offered,
    /// It can act as an OpenID identity for other sites.
    pub sign_in: Offered,
}

/// A mailbox. The protocol detail (IMAP extensions, folder roles) stays with the mail engine;
/// this is what an app can plan around.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MailCap {
    /// Reading and changing messages.
    pub access: Access,
    /// Sending.
    pub send: Offered,
    /// How new mail becomes known.
    pub delta: Delta,
    /// The protocol family that reaches it.
    pub transport: MailTransport,
    /// How messages are grouped.
    pub labels: LabelModel,
}

/// The protocol a mailbox is reached by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailTransport {
    /// IMAP (with SMTP for sending).
    Imap,
    /// JMAP.
    Jmap,
    /// Microsoft Graph.
    Graph,
    /// The Gmail REST API.
    GmailApi,
    /// POP3 (with SMTP for sending).
    Pop3,
}

/// Whether a message lives in one folder or carries several labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelModel {
    /// One folder per message.
    Folders,
    /// Any number of labels per message.
    Labels,
}
