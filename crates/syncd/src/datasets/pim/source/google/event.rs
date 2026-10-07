//! A Google Calendar event as an iCalendar: pure, no I/O.
//!
//! One event is one `VCALENDAR` with one `VEVENT`, which is what the vdir holds per file.
//!
//! | Google | iCalendar |
//! |---|---|
//! | `iCalUID`, else `id` | `UID` (an exception shares its series' UID) |
//! | `summary`, `description`, `location` | `SUMMARY`, `DESCRIPTION`, `LOCATION` |
//! | `start`, `end` with `date` | `DTSTART;VALUE=DATE`, `DTEND;VALUE=DATE` (the end is exclusive in both) |
//! | `start`, `end` with `dateTime` | `...Z` (the instant, in UTC); `TZID=<timeZone>` and the wall-clock time for a member of a series, so the rule keeps to the zone's clock across daylight saving |
//! | `recurrence` | `RRULE`, `EXRULE`, `RDATE`, `EXDATE` lines as Google wrote them |
//! | `originalStartTime` of an exception | `RECURRENCE-ID`, in the form the series' `DTSTART` has |
//! | `status` | `STATUS` |
//! | `visibility`, `transparency` | `CLASS`, `TRANSP` |
//! | `created`, `updated`, `sequence` | `CREATED`, `DTSTAMP` and `LAST-MODIFIED`, `SEQUENCE` |
//! | `organizer`, `attendees` | `ORGANIZER`, `ATTENDEE` (`CN`, `ROLE`, `PARTSTAT`, `CUTYPE`) |
//!
//! Not carried: reminders, conference data, attachments, colours, extended properties. A series
//! master and each of its exceptions are separate events in Google and separate files here,
//! sharing the `UID`; the master has no `EXDATE` for an exception Google sent as its own event.
//! A single event is written in UTC rather than in its `timeZone` on purpose: the instant is
//! exact whichever way Google states the offset, and a series needs the zone only for the rule.

use super::super::graph::convert::{escape, fold, param};
use super::time::{Stamp, compact_date};
use serde::Deserialize;

/// Why an event could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    /// No start time.
    #[error("the event has no start")]
    NoStart,
    /// A date or date-time that is not RFC 3339.
    #[error("a date or time could not be read")]
    BadTime,
}

/// A start, end or original start.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct When {
    /// An all-day event's date, `2026-12-24`.
    pub date: Option<String>,
    /// A timed event's RFC 3339 time, with its offset.
    #[serde(rename = "dateTime")]
    pub date_time: Option<String>,
    /// Its IANA zone, when Google states one.
    #[serde(rename = "timeZone")]
    pub time_zone: Option<String>,
}

/// An organizer.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Principal {
    /// Their address.
    pub email: Option<String>,
    /// Their name.
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// One attendee.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Attendee {
    /// Their address.
    pub email: Option<String>,
    /// Their name.
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    /// `needsAction`, `declined`, `tentative` or `accepted`.
    #[serde(rename = "responseStatus")]
    pub response_status: Option<String>,
    /// Whether attendance is optional.
    pub optional: Option<bool>,
    /// Whether this is a room or other resource.
    pub resource: Option<bool>,
}

/// An event as `events.list` and `events.get` give it, as far as the converter reads it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Event {
    /// Its id in the calendar.
    pub id: String,
    /// Its version.
    pub etag: Option<String>,
    /// `confirmed`, `tentative` or `cancelled`.
    pub status: Option<String>,
    /// The title.
    pub summary: Option<String>,
    /// The description.
    pub description: Option<String>,
    /// The place, as text.
    pub location: Option<String>,
    /// When it starts.
    pub start: Option<When>,
    /// When it ends.
    pub end: Option<When>,
    /// The series' rule lines.
    #[serde(default)]
    pub recurrence: Vec<String>,
    /// The series master an exception belongs to.
    #[serde(rename = "recurringEventId")]
    pub recurring_event_id: Option<String>,
    /// Where an exception would have been in its series.
    #[serde(rename = "originalStartTime")]
    pub original_start: Option<When>,
    /// Its iCalendar UID.
    #[serde(rename = "iCalUID")]
    pub ical_uid: Option<String>,
    /// When it was made.
    pub created: Option<String>,
    /// When it last changed.
    pub updated: Option<String>,
    /// Its revision number.
    pub sequence: Option<u32>,
    /// `opaque` or `transparent`.
    pub transparency: Option<String>,
    /// `default`, `public`, `private` or `confidential`.
    pub visibility: Option<String>,
    /// Who organises it.
    pub organizer: Option<Principal>,
    /// Who is invited.
    #[serde(default)]
    pub attendees: Vec<Attendee>,
}

impl Event {
    /// Whether this is a deletion's stub (or a cancelled event): the item is gone.
    pub fn is_cancelled(&self) -> bool {
        self.status.as_deref() == Some("cancelled")
    }

    /// Whether the event belongs to a series, as its master or as an exception.
    fn in_series(&self) -> bool {
        !self.recurrence.is_empty() || self.recurring_event_id.is_some()
    }

    /// The UID the event is known by.
    pub fn uid(&self) -> &str {
        self.ical_uid
            .as_deref()
            .filter(|u| !u.is_empty())
            .unwrap_or(&self.id)
    }
}

fn is_utc(zone: &str) -> bool {
    ["utc", "etc/utc", "gmt", "etc/gmt", "z"]
        .iter()
        .any(|name| zone.eq_ignore_ascii_case(name))
}

/// A `DTSTART`, `DTEND` or `RECURRENCE-ID` line for `when`.
fn moment(name: &str, when: &When, in_series: bool) -> Result<String, ConvertError> {
    if let Some(date) = when.date.as_deref() {
        return Ok(format!(
            "{name};VALUE=DATE:{}",
            compact_date(date).ok_or(ConvertError::BadTime)?
        ));
    }
    let text = when.date_time.as_deref().ok_or(ConvertError::NoStart)?;
    let stamp = Stamp::parse(text).ok_or(ConvertError::BadTime)?;
    Ok(match when.time_zone.as_deref().filter(|z| !z.is_empty()) {
        Some(zone) if in_series && !is_utc(zone) => {
            format!("{name};TZID={}:{}", param(zone), stamp.local())
        }
        _ => format!("{name}:{}", stamp.utc()),
    })
}

fn stamp(text: Option<&str>) -> Option<String> {
    Stamp::parse(text?).map(|s| s.utc())
}

fn mailto(address: &str) -> Option<String> {
    Some(address)
        .filter(|a| !a.is_empty() && !a.contains(char::is_whitespace))
        .map(|a| format!("mailto:{a}"))
}

fn common_name(name: Option<&str>) -> String {
    name.filter(|n| !n.is_empty())
        .map(|n| format!(";CN={}", param(n)))
        .unwrap_or_default()
}

fn attendee_line(attendee: &Attendee) -> Option<String> {
    let uri = mailto(attendee.email.as_deref()?)?;
    let (role, kind) = match (attendee.resource, attendee.optional) {
        (Some(true), _) => ("NON-PARTICIPANT", ";CUTYPE=RESOURCE"),
        (_, Some(true)) => ("OPT-PARTICIPANT", ""),
        _ => ("REQ-PARTICIPANT", ""),
    };
    let partstat = match attendee.response_status.as_deref() {
        Some("accepted") => "ACCEPTED",
        Some("declined") => "DECLINED",
        Some("tentative") => "TENTATIVE",
        _ => "NEEDS-ACTION",
    };
    Some(format!(
        "ATTENDEE{}{kind};ROLE={role};PARTSTAT={partstat}:{uri}",
        common_name(attendee.display_name.as_deref())
    ))
}

/// The recurrence lines that are rules and dates, without a stray line break: nothing else is
/// let into the calendar from this field.
fn rule_lines(event: &Event) -> impl Iterator<Item = String> + '_ {
    event.recurrence.iter().filter_map(|line| {
        let line: String = line.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
        let name = line.split([':', ';']).next()?.to_ascii_uppercase();
        matches!(name.as_str(), "RRULE" | "EXRULE" | "RDATE" | "EXDATE").then_some(line)
    })
}

/// The iCalendar of `event`.
pub fn to_ics(event: &Event) -> Result<String, ConvertError> {
    let series = event.in_series();
    let start = event.start.as_ref().ok_or(ConvertError::NoStart)?;
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "VERSION:2.0".into(),
        "PRODID:-//porter//syncd google calendar//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", escape(event.uid())),
    ];
    let modified = stamp(event.updated.as_deref());
    let created = stamp(event.created.as_deref());
    lines.push(format!(
        "DTSTAMP:{}",
        modified
            .as_ref()
            .or(created.as_ref())
            .map_or("19700101T000000Z", String::as_str)
    ));
    lines.extend(created.map(|s| format!("CREATED:{s}")));
    lines.extend(modified.map(|s| format!("LAST-MODIFIED:{s}")));
    lines.extend(event.sequence.map(|n| format!("SEQUENCE:{n}")));
    lines.extend(
        event
            .summary
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| format!("SUMMARY:{}", escape(v))),
    );
    lines.push(moment("DTSTART", start, series)?);
    if let Some(end) = event.end.as_ref() {
        lines.push(moment("DTEND", end, series)?);
    }
    lines.extend(rule_lines(event));
    if let (Some(_), Some(original)) = (event.recurring_event_id.as_ref(), &event.original_start) {
        lines.push(moment("RECURRENCE-ID", original, true)?);
    }
    for (name, value) in [
        ("LOCATION", &event.location),
        ("DESCRIPTION", &event.description),
    ] {
        lines.extend(
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| format!("{name}:{}", escape(v))),
        );
    }
    lines.push(
        match event.status.as_deref() {
            Some("cancelled") => "STATUS:CANCELLED",
            Some("tentative") => "STATUS:TENTATIVE",
            _ => "STATUS:CONFIRMED",
        }
        .into(),
    );
    match event.visibility.as_deref() {
        Some("private") => lines.push("CLASS:PRIVATE".into()),
        Some("confidential") => lines.push("CLASS:CONFIDENTIAL".into()),
        Some("public") => lines.push("CLASS:PUBLIC".into()),
        _ => {}
    }
    match event.transparency.as_deref() {
        Some("transparent") => lines.push("TRANSP:TRANSPARENT".into()),
        Some("opaque") => lines.push("TRANSP:OPAQUE".into()),
        _ => {}
    }
    if let Some(organizer) = event.organizer.as_ref().and_then(|o| {
        let uri = mailto(o.email.as_deref()?)?;
        Some(format!(
            "ORGANIZER{}:{uri}",
            common_name(o.display_name.as_deref())
        ))
    }) {
        lines.push(organizer);
    }
    lines.extend(event.attendees.iter().filter_map(attendee_line));
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    Ok(lines.iter().map(|l| fold(l)).collect())
}

#[cfg(test)]
mod tests;
