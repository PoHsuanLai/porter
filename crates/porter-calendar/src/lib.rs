//! One calendar store for every porter client: the calendars an account or a desktop holds, and
//! an event put into one, found in one by its UID, and taken out of one.
//!
//! [`CalendarStore`] is the seam; each backend implements it over the transport its store
//! speaks:
//!
//! - [`CalDav`]: CalDAV (RFC 4791) over the HTTP seam, through dav-proto and porter-dav. An event
//!   is a resource holding its iCalendar text; a write carries `If-Match` or `If-None-Match: *`,
//!   so it never overwrites a change it has not seen.
//! - [`Graph`]: Microsoft Graph's calendars. Graph takes JSON and keeps an `iCalUId` of its own,
//!   so an event put here carries its UID in an extended property, and is found by either.
//! - [`Google`]: the Google Calendar API, whose `import` keeps the event's own `iCalUID`.
//! - [`Jmap`]: JMAP for Calendars, with the event as JSCalendar.
//! - `Eds` (feature `eds`): Evolution Data Server over the session bus, the calendar store of
//!   GNOME and most Linux desktops.
//!
//! The iCalendar data stays the caller's: an [`Event`] carries its text whole, for the stores
//! that keep iCalendar, and the few fields a JSON store needs beside it, which the caller read
//! from that text with its own parser. Nothing here parses or rewrites iCalendar. Credentials are
//! the HTTP seam's (a relay adds them), as for every porter family.

mod caldav;
mod error;
mod event;
mod google;
mod graph;
mod jmap;
mod json;
mod time;

#[cfg(feature = "eds")]
pub mod eds;

pub use caldav::CalDav;
pub use error::CalendarError;
pub use event::{CalendarRef, Event, Stored, When};
pub use google::Google;
pub use graph::Graph;
pub use jmap::Jmap;
pub use time::{Date, Instant};

/// The HTTP seam the HTTP backends send through, at the version this crate names.
pub use porter_http;

use std::future::Future;

/// A store of calendars and their events.
///
/// Every method is one exchange with the store, or a few: nothing is cached between calls, so a
/// caller that puts and then finds sees the store's own answer.
pub trait CalendarStore: Send + Sync {
    /// Every calendar the store holds, in its order.
    fn calendars(&self) -> impl Future<Output = Result<Vec<CalendarRef>, CalendarError>> + Send;

    /// The event with `uid` in `calendar`, `None` when there is none.
    fn find(
        &self,
        calendar: &str,
        uid: &str,
    ) -> impl Future<Output = Result<Option<Stored>, CalendarError>> + Send;

    /// Put `event` into `calendar`: a new one, or the one with its UID replaced. The answer is
    /// the event as stored.
    fn put(
        &self,
        calendar: &str,
        event: &Event,
    ) -> impl Future<Output = Result<Stored, CalendarError>> + Send;

    /// Take the event with `uid` out of `calendar`. `true` when there was one.
    fn remove(
        &self,
        calendar: &str,
        uid: &str,
    ) -> impl Future<Output = Result<bool, CalendarError>> + Send;
}
