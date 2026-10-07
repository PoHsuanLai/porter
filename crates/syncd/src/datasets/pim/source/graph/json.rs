//! What Graph's calendar JSON says, read as far as the converter and the feed need. Every field
//! is optional: Graph leaves out what an event does not have, and a delta item for a changed
//! event may carry fewer properties than a full read.

use serde::Deserialize;
use serde::de::IgnoredAny;

/// `GET /me/calendars`, one page.
#[derive(Debug, Deserialize)]
pub struct CalendarPage {
    #[serde(default)]
    pub value: Vec<Calendar>,
    #[serde(rename = "@odata.nextLink")]
    pub next: Option<String>,
}

/// One calendar.
#[derive(Debug, Deserialize)]
pub struct Calendar {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "hexColor")]
    pub hex_color: Option<String>,
    pub color: Option<String>,
}

/// One page of `events/delta`.
#[derive(Debug, Deserialize)]
pub struct EventPage {
    #[serde(default)]
    pub value: Vec<Event>,
    #[serde(rename = "@odata.nextLink")]
    pub next: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub delta: Option<String>,
}

/// A date and time as Graph writes it: local to `time_zone`, no offset.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct When {
    #[serde(rename = "dateTime")]
    pub date_time: String,
    #[serde(rename = "timeZone")]
    pub time_zone: Option<String>,
}

/// An event body.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Body {
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub content: Option<String>,
}

/// A place.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Location {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// An email address with its display name.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Address {
    pub name: Option<String>,
    pub address: Option<String>,
}

/// A person on the event.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Person {
    #[serde(rename = "emailAddress")]
    pub email: Option<Address>,
}

/// An attendee.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Attendee {
    #[serde(rename = "type")]
    pub role: Option<String>,
    pub status: Option<Status>,
    #[serde(rename = "emailAddress")]
    pub email: Option<Address>,
}

/// An attendee's answer.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Status {
    pub response: Option<String>,
}

/// `patternedRecurrence`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Recurrence {
    pub pattern: Option<Pattern>,
    pub range: Option<Range>,
}

/// `recurrencePattern`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Pattern {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub interval: Option<u32>,
    #[serde(rename = "daysOfWeek", default)]
    pub days_of_week: Vec<String>,
    #[serde(rename = "dayOfMonth")]
    pub day_of_month: Option<u32>,
    pub month: Option<u32>,
    pub index: Option<String>,
    #[serde(rename = "firstDayOfWeek")]
    pub first_day_of_week: Option<String>,
}

/// `recurrenceRange`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Range {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
    #[serde(rename = "numberOfOccurrences")]
    pub occurrences: Option<u32>,
}

/// An event, or the stub of a deleted one (`@removed`).
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub id: String,
    #[serde(rename = "@removed")]
    pub removed: Option<IgnoredAny>,
    #[serde(rename = "iCalUId")]
    pub ical_uid: Option<String>,
    #[serde(rename = "changeKey")]
    pub change_key: Option<String>,
    #[serde(rename = "lastModifiedDateTime")]
    pub modified: Option<String>,
    #[serde(rename = "createdDateTime")]
    pub created: Option<String>,
    pub subject: Option<String>,
    pub body: Option<Body>,
    #[serde(rename = "bodyPreview")]
    pub body_preview: Option<String>,
    pub start: Option<When>,
    pub end: Option<When>,
    #[serde(rename = "originalStart")]
    pub original_start: Option<String>,
    #[serde(rename = "isAllDay")]
    pub is_all_day: Option<bool>,
    #[serde(rename = "isCancelled")]
    pub is_cancelled: Option<bool>,
    #[serde(rename = "showAs")]
    pub show_as: Option<String>,
    pub sensitivity: Option<String>,
    pub location: Option<Location>,
    pub organizer: Option<Person>,
    #[serde(default)]
    pub attendees: Vec<Attendee>,
    pub recurrence: Option<Recurrence>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "seriesMasterId")]
    pub series_master_id: Option<String>,
}
