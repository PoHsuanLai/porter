//! What goes into a calendar store and what comes back out.

use crate::time::{Date, Instant};

/// One calendar of a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarRef {
    /// What the store knows it by, and what [`crate::CalendarStore`]'s methods take: a CalDAV
    /// collection's URL, a Graph, Google or JMAP calendar id, an EDS source's UID.
    pub id: String,
    /// The name a person gave it.
    pub name: String,
    /// Its colour as the store writes it (`#0082c9`, `#0082c9FF`), when it has one.
    pub color: Option<String>,
    /// Whether events may be put into it. A store that does not say is taken as writable.
    pub writable: bool,
}

/// An event to put into a store.
///
/// `ics` is the whole iCalendar object (a `VCALENDAR` with the event and any `VTIMEZONE` it
/// names), as the caller wrote or received it: what CalDAV and Evolution Data Server store. The
/// other fields are the same event read by the caller's own parser, for the stores that take
/// JSON (Graph, Google, JMAP), which keep only these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The event's `UID`.
    pub uid: String,
    /// The iCalendar text.
    pub ics: String,
    /// `SUMMARY`.
    pub summary: String,
    pub when: When,
    /// `LOCATION`.
    pub location: Option<String>,
    /// `DESCRIPTION`.
    pub description: Option<String>,
}

/// When an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Whole days, `last` included.
    AllDay { first: Date, last: Date },
    /// From `start` until `end`.
    Timed { start: Instant, end: Instant },
}

/// An event as a store holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// The calendar it is in.
    pub calendar: String,
    /// Its `UID`.
    pub uid: String,
    /// What the store knows it by: a CalDAV resource's URL, a Graph, Google or JMAP event id, the
    /// UID again for EDS.
    pub id: String,
    /// Its version as the store last said it (an etag), when the store says one.
    pub version: Option<String>,
    /// Its iCalendar text, from a store that keeps one.
    pub ics: Option<String>,
}
