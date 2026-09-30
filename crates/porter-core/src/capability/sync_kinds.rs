//! KeyValue and Push (design/31 §2.2): the kinds syncd plans with.

use super::terms::Delta;
use crate::units::Bytes;
use serde::{Deserialize, Serialize};

/// Small encrypted items (the settings log, design/31 §6.4), served natively or by syncd over
/// any Storage account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyValueCap {
    /// How changes become known.
    pub delta: Delta,
    /// The largest item.
    pub max_item: Bytes,
}

/// A push channel, derived from the other kinds so the scheduler can plan its wake-ups.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PushCap {
    /// How the server reaches us.
    pub channel: PushChannel,
}

/// The mechanism behind a push capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushChannel {
    /// IMAP IDLE.
    ImapIdle,
    /// JMAP's EventSource.
    JmapEventSource,
    /// An HTTP long-poll (Dropbox, Box).
    LongPoll,
    /// A WebSocket (Nextcloud `notify_push`).
    WebSocket,
}
