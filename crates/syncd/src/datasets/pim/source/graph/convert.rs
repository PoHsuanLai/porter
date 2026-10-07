//! A Graph event as an iCalendar: pure, no I/O.
//!
//! One event is one `VCALENDAR` with one `VEVENT`, which is what the vdir holds per file.
//!
//! | Graph | iCalendar |
//! |---|---|
//! | `iCalUId`, else `seriesMasterId`, else `id` | `UID` (an exception shares its series' UID) |
//! | `subject` | `SUMMARY` |
//! | `start`, `end` with `timeZone` | `DTSTART`, `DTEND`: `...Z` for UTC, `TZID=` for a zone, a date for `isAllDay` |
//! | `location.displayName` | `LOCATION` |
//! | `body` (text; HTML reduced to text), else `bodyPreview` | `DESCRIPTION` |
//! | `recurrence` | `RRULE` ([`super::recur`]); a pattern it cannot say becomes a `COMMENT` |
//! | `originalStart` of an exception or occurrence | `RECURRENCE-ID` |
//! | `isCancelled`, `showAs: tentative` | `STATUS:CANCELLED`, `STATUS:TENTATIVE`, else `CONFIRMED` |
//! | `showAs: free` | `TRANSP:TRANSPARENT` |
//! | `sensitivity` | `CLASS` |
//! | `organizer`, `attendees` | `ORGANIZER`, `ATTENDEE` (`ROLE`, `PARTSTAT`, `CN`) |
//!
//! Not carried: reminders, categories, online meeting links, attachments. A time zone this
//! build has no IANA name for is written floating with a `COMMENT` naming the Graph zone.
//! A series' exceptions are separate events in Graph and separate files here, sharing the
//! master's `UID` with a `RECURRENCE-ID`; the master has no `EXDATE` for them.

use super::json::{Address, Attendee, Event, When};
use super::recur::{Rule, compact_date, rule};
use super::zones::{Zone, zone};

/// Why an event could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    /// A deleted stub, not an event.
    #[error("the event was removed")]
    Removed,
    /// No start time.
    #[error("the event has no start")]
    NoStart,
    /// A date-time that is not `YYYY-MM-DDTHH:MM:SS`.
    #[error("a date-time could not be read")]
    BadDateTime,
}

const CRLF: &str = "\r\n";

/// A `TEXT` value: backslash, semicolon, comma and newlines escaped.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\r' => {}
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// A content line folded at 75 octets, never inside a character, and ended with CRLF.
pub fn fold(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut width = 0;
    let mut limit = 75;
    for c in line.chars() {
        let n = c.len_utf8();
        if width + n > limit && width > 0 {
            out.push_str(CRLF);
            out.push(' ');
            width = 0;
            limit = 74;
        }
        out.push(c);
        width += n;
    }
    out.push_str(CRLF);
    out
}

/// A parameter value, quoted when it holds a character that would end it.
pub(in crate::datasets::pim::source) fn param(text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n'))
        .collect();
    match clean.contains([':', ';', ',']) {
        true => format!("\"{clean}\""),
        false => clean,
    }
}

/// `20261006T100000` from `2026-10-06T10:00:00.0000000`.
fn local(date_time: &str) -> Result<String, ConvertError> {
    let date = compact_date(date_time).ok_or(ConvertError::BadDateTime)?;
    let time = date_time
        .get(11..19)
        .filter(|t| {
            t.as_bytes().iter().enumerate().all(|(at, b)| match at {
                2 | 5 => *b == b':',
                _ => b.is_ascii_digit(),
            })
        })
        .ok_or(ConvertError::BadDateTime)?;
    Ok(format!("{date}T{}", time.replace(':', "")))
}

/// A DTSTART or DTEND line for `when`; the zone's comment, if it needs one.
fn moment(
    name: &str,
    when: &When,
    all_day: bool,
    comments: &mut Vec<String>,
) -> Result<String, ConvertError> {
    if all_day {
        let date = compact_date(&when.date_time).ok_or(ConvertError::BadDateTime)?;
        return Ok(format!("{name};VALUE=DATE:{date}"));
    }
    let time = local(&when.date_time)?;
    Ok(match zone(when.time_zone.as_deref()) {
        Zone::Utc => format!("{name}:{time}Z"),
        Zone::Iana(tz) => format!("{name};TZID={tz}:{time}"),
        Zone::Unknown(tz) => {
            let note = format!("Time zone {tz} is not known here; times are floating");
            if !comments.contains(&note) {
                comments.push(note);
            }
            format!("{name}:{time}")
        }
    })
}

/// `20261005T123000Z` from a UTC stamp Graph wrote (`2026-10-05T12:30:00.1234567Z`).
fn stamp(text: &str) -> Option<String> {
    local(text).ok().map(|t| format!("{t}Z"))
}

/// Text from an HTML body: tags dropped, block ends as newlines, the common entities decoded.
pub fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut tag = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let name: String = tag
                    .trim_start_matches('/')
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_ascii_lowercase();
                let closing = tag.starts_with('/');
                if name == "br" || (closing && matches!(name.as_str(), "p" | "div" | "li" | "tr")) {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    // A `<head><style>` block leaves its CSS; a body that is a whole page is rare enough.
    let lines: Vec<&str> = decoded.lines().map(str::trim_end).collect();
    lines.join("\n").trim().to_owned()
}

fn description(event: &Event) -> Option<String> {
    let body = event.body.as_ref().and_then(|b| {
        let content = b.content.as_deref()?;
        Some(match b.content_type.as_deref() {
            Some(t) if t.eq_ignore_ascii_case("html") => html_text(content),
            _ => content.replace("\r\n", "\n").trim().to_owned(),
        })
    });
    body.filter(|t| !t.is_empty()).or_else(|| {
        event
            .body_preview
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    })
}

fn mailto(address: &Address) -> Option<String> {
    address
        .address
        .as_deref()
        .filter(|a| !a.is_empty() && !a.contains(char::is_whitespace))
        .map(|a| format!("mailto:{a}"))
}

fn common_name(address: &Address) -> String {
    address
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .map(|n| format!(";CN={}", param(n)))
        .unwrap_or_default()
}

fn attendee_line(attendee: &Attendee) -> Option<String> {
    let address = attendee.email.as_ref()?;
    let uri = mailto(address)?;
    let role = match attendee.role.as_deref() {
        Some("optional") => "OPT-PARTICIPANT",
        Some("resource") => "NON-PARTICIPANT",
        _ => "REQ-PARTICIPANT",
    };
    let partstat = match attendee.status.as_ref().and_then(|s| s.response.as_deref()) {
        Some("accepted" | "organizer") => "ACCEPTED",
        Some("declined") => "DECLINED",
        Some("tentativelyAccepted") => "TENTATIVE",
        _ => "NEEDS-ACTION",
    };
    Some(format!(
        "ATTENDEE{};ROLE={role};PARTSTAT={partstat}:{uri}",
        common_name(address)
    ))
}

fn recurrence_id(event: &Event, all_day: bool) -> Option<String> {
    let original = event.original_start.as_deref()?;
    match all_day {
        true => Some(format!(
            "RECURRENCE-ID;VALUE=DATE:{}",
            compact_date(original)?
        )),
        false => Some(format!("RECURRENCE-ID:{}", stamp(original)?)),
    }
}

/// The UID the event is known by.
pub fn uid(event: &Event) -> &str {
    [
        event.ical_uid.as_deref(),
        event.series_master_id.as_deref(),
        Some(event.id.as_str()),
    ]
    .into_iter()
    .flatten()
    .find(|u| !u.is_empty())
    .unwrap_or(&event.id)
}

/// The iCalendar of `event`.
pub fn to_ics(event: &Event) -> Result<String, ConvertError> {
    if event.removed.is_some() {
        return Err(ConvertError::Removed);
    }
    let all_day = event.is_all_day == Some(true);
    let start = event.start.as_ref().ok_or(ConvertError::NoStart)?;
    let mut comments = Vec::new();
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "VERSION:2.0".into(),
        "PRODID:-//porter//syncd graph calendar//EN".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", escape(uid(event))),
    ];
    let dtstamp = event
        .modified
        .as_deref()
        .or(event.created.as_deref())
        .and_then(stamp)
        .unwrap_or_else(|| "19700101T000000Z".to_owned());
    lines.push(format!("DTSTAMP:{dtstamp}"));
    lines.extend(
        event
            .created
            .as_deref()
            .and_then(stamp)
            .map(|s| format!("CREATED:{s}")),
    );
    lines.extend(
        event
            .modified
            .as_deref()
            .and_then(stamp)
            .map(|s| format!("LAST-MODIFIED:{s}")),
    );
    if let Some(subject) = event.subject.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("SUMMARY:{}", escape(subject)));
    }
    lines.push(moment("DTSTART", start, all_day, &mut comments)?);
    if let Some(end) = event.end.as_ref() {
        lines.push(moment("DTEND", end, all_day, &mut comments)?);
    }
    if let Some(recurrence) = event.recurrence.as_ref() {
        match rule(recurrence, all_day) {
            Rule::Rrule(rrule) => lines.push(format!("RRULE:{rrule}")),
            Rule::Unsupported(why) => comments.push(format!(
                "{why}; this is the series master and its occurrences are as Graph sends them"
            )),
        }
    }
    let is_exception = matches!(event.kind.as_deref(), Some("exception" | "occurrence"));
    if is_exception {
        match recurrence_id(event, all_day) {
            Some(line) => lines.push(line),
            None => comments.push("An occurrence of a series with no original start".into()),
        }
    }
    if let Some(place) = event
        .location
        .as_ref()
        .and_then(|l| l.display_name.as_deref())
        .filter(|p| !p.is_empty())
    {
        lines.push(format!("LOCATION:{}", escape(place)));
    }
    if let Some(text) = description(event) {
        lines.push(format!("DESCRIPTION:{}", escape(&text)));
    }
    let status = match (event.is_cancelled == Some(true), event.show_as.as_deref()) {
        (true, _) => "CANCELLED",
        (false, Some("tentative")) => "TENTATIVE",
        _ => "CONFIRMED",
    };
    lines.push(format!("STATUS:{status}"));
    match event.sensitivity.as_deref() {
        Some("private") => lines.push("CLASS:PRIVATE".into()),
        Some("confidential") => lines.push("CLASS:CONFIDENTIAL".into()),
        _ => {}
    }
    if event.show_as.as_deref() == Some("free") {
        lines.push("TRANSP:TRANSPARENT".into());
    }
    if let Some(organizer) = event
        .organizer
        .as_ref()
        .and_then(|o| o.email.as_ref())
        .and_then(|a| Some(format!("ORGANIZER{}:{}", common_name(a), mailto(a)?)))
    {
        lines.push(organizer);
    }
    lines.extend(event.attendees.iter().filter_map(attendee_line));
    lines.extend(comments.iter().map(|c| format!("COMMENT:{}", escape(c))));
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    Ok(lines.iter().map(|l| fold(l)).collect())
}

#[cfg(test)]
mod tests;
