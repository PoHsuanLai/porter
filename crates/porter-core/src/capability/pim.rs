//! Calendar, Contacts, Tasks and Notes (design/31 §2.2).

use super::terms::{Access, Delta, Offered};
use serde::{Deserialize, Serialize};

/// A calendar, address book or task list store (one shape for the three kinds).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PimCap {
    /// Reading and changing entries.
    pub access: Access,
    /// How changes become known.
    pub delta: Delta,
    /// The protocol family that reaches it.
    pub transport: PimTransport,
    /// Several named collections (calendars, address books, lists) rather than one.
    pub collections: Offered,
}

/// The protocol a PIM store is reached by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PimTransport {
    /// CalDAV (calendars and VTODO tasks).
    #[serde(rename = "caldav")]
    CalDav,
    /// CardDAV (contacts).
    #[serde(rename = "carddav")]
    CardDav,
    /// JMAP.
    Jmap,
    /// Microsoft Graph.
    Graph,
    /// Google's Calendar, People or Tasks API.
    GoogleApi,
}

/// A notes store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NotesCap {
    /// Reading and changing notes.
    pub access: Access,
    /// How changes become known (OneNote has none, C11).
    pub delta: Delta,
    /// The protocol family that reaches it.
    pub transport: NotesTransport,
}

/// The protocol a notes store is reached by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotesTransport {
    /// The Nextcloud Notes API.
    NextcloudNotes,
    /// Microsoft OneNote through Graph.
    OneNote,
    /// Notes kept as messages in an IMAP folder.
    ImapNotes,
}
