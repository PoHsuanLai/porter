//! The calendar half of the fake Google PIM APIs: `calendarList`, a calendar's `events.list`
//! (a `syncToken` handed out on the last page, `nextPageToken` between pages, cancelled stubs for
//! deleted events once there is a token, `410 fullSyncRequired` for a token the server dropped)
//! and `events.get`. Events are kept as the JSON a test gives; the fake adds `id`, `etag`,
//! `updated`, and a default `status` and `iCalUID`.

use super::pim::{error, numbers, stamp};
use crate::http::{Request, Response, percent_decode};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Calendar {
    id: String,
    summary: String,
    color: String,
}

#[derive(Debug, Clone)]
struct Row {
    calendar: String,
    id: String,
    event: Value,
    seq: u64,
    removed: bool,
}

/// The calendars and their events.
#[derive(Debug, Default)]
pub struct Calendars {
    calendars: Vec<Calendar>,
    rows: Vec<Row>,
    seq: u64,
    /// Sync tokens older than this are dropped.
    expired_before: u64,
}

impl Calendars {
    /// Whether any calendar is set (until one is, the probe's one primary calendar answers).
    pub fn is_empty(&self) -> bool {
        self.calendars.is_empty()
    }

    /// Adds the calendar `id`, or renames and recolours it.
    pub fn set_calendar(&mut self, id: &str, summary: &str, color: &str) {
        let next = Calendar {
            id: id.to_owned(),
            summary: summary.to_owned(),
            color: color.to_owned(),
        };
        match self.calendars.iter_mut().find(|c| c.id == id) {
            Some(held) => *held = next,
            None => self.calendars.push(next),
        }
    }

    /// Removes a calendar and its events.
    pub fn remove_calendar(&mut self, id: &str) {
        self.calendars.retain(|c| c.id != id);
        self.rows.retain(|r| r.calendar != id);
    }

    /// Creates or replaces an event.
    pub fn put_event(&mut self, calendar: &str, id: &str, mut event: Value) {
        self.seq += 1;
        if let Value::Object(map) = &mut event {
            map.insert("kind".into(), json!("calendar#event"));
            map.insert("id".into(), json!(id));
            map.insert("etag".into(), json!(format!("\"{}\"", self.seq * 1000)));
            map.insert("updated".into(), json!(stamp(self.seq)));
            map.entry("status").or_insert_with(|| json!("confirmed"));
            map.entry("iCalUID")
                .or_insert_with(|| json!(format!("{id}@google.com")));
        }
        let row = Row {
            calendar: calendar.to_owned(),
            id: id.to_owned(),
            event,
            seq: self.seq,
            removed: false,
        };
        match self
            .rows
            .iter_mut()
            .find(|r| r.calendar == calendar && r.id == id)
        {
            Some(held) => *held = row,
            None => self.rows.push(row),
        }
    }

    /// Deletes an event, leaving the cancelled stub the next incremental listing reports.
    pub fn remove_event(&mut self, calendar: &str, id: &str) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.calendar == calendar && r.id == id)
        {
            row.removed = true;
            row.seq = seq;
            row.event = json!({
                "kind": "calendar#event", "id": id, "status": "cancelled",
                "etag": format!("\"{}\"", seq * 1000),
            });
        }
    }

    /// Drops every sync token handed out so far.
    pub fn expire(&mut self) {
        self.expired_before = self.seq + 1;
        self.seq += 1;
    }

    /// The live events of a calendar: id and JSON.
    pub fn events(&self, calendar: &str) -> Vec<(String, Value)> {
        self.rows
            .iter()
            .filter(|r| r.calendar == calendar && !r.removed)
            .map(|r| (r.id.clone(), r.event.clone()))
            .collect()
    }

    /// A small calendar set a test can start from: `cal-personal` with a timed event, a weekly
    /// series with one moved occurrence, an all-day event and a cancelled one, and `cal-work`
    /// with one event.
    pub fn seed(&mut self) {
        let at =
            |date_time: &str| json!({"dateTime": date_time, "timeZone": "America/Los_Angeles"});
        self.set_calendar("cal-personal", "Personal", "#9fe1e7");
        self.set_calendar("cal-work", "Work", "#f83a22");
        self.put_event(
            "cal-personal",
            "ev-dentist",
            json!({
                "iCalUID": "uid-dentist@google.com", "summary": "Dentist", "location": "Main St 1",
                "description": "Bring the card",
                "start": at("2026-10-06T10:00:00-07:00"), "end": at("2026-10-06T11:00:00-07:00"),
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-standup",
            json!({
                "iCalUID": "uid-standup@google.com", "summary": "Standup",
                "start": at("2026-10-05T09:00:00-07:00"), "end": at("2026-10-05T09:15:00-07:00"),
                "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-standup_20261012T160000Z",
            json!({
                "iCalUID": "uid-standup@google.com", "summary": "Standup (moved)",
                "recurringEventId": "ev-standup",
                "originalStartTime": at("2026-10-12T09:00:00-07:00"),
                "start": at("2026-10-12T10:00:00-07:00"), "end": at("2026-10-12T10:15:00-07:00"),
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-holiday",
            json!({
                "iCalUID": "uid-holiday@google.com", "summary": "Holiday",
                "start": {"date": "2026-12-24"}, "end": {"date": "2026-12-26"},
            }),
        );
        self.put_event(
            "cal-work",
            "ev-review",
            json!({
                "iCalUID": "uid-review@google.com", "summary": "Design review",
                "start": {"dateTime": "2026-10-07T13:00:00Z"}, "end": {"dateTime": "2026-10-07T14:00:00Z"},
                "organizer": {"email": "ada@example.test", "displayName": "Ada"},
                "attendees": [{"email": "grace@example.test", "displayName": "Grace",
                               "responseStatus": "accepted"}],
            }),
        );
    }

    fn list(&self, request: &Request) -> Response {
        let page = request
            .query_value("maxResults")
            .and_then(|n| n.parse().ok())
            .unwrap_or(250usize)
            .max(1);
        let offset = request
            .query_value("pageToken")
            .and_then(|t| numbers(&t, "cl").and_then(|n| n.first().copied()))
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let all: Vec<Value> = self
            .calendars
            .iter()
            .map(|c| {
                json!({"kind": "calendar#calendarListEntry", "id": c.id, "summary": c.summary,
                       "backgroundColor": c.color, "foregroundColor": "#000000",
                       "accessRole": "owner", "selected": true})
            })
            .collect();
        let items: Vec<Value> = all.iter().skip(offset).take(page).cloned().collect();
        let mut body = json!({"kind": "calendar#calendarList", "items": items});
        if offset + page < all.len() {
            body["nextPageToken"] = json!(format!("cl{}", offset + page));
        }
        Response::json(200, &body)
    }

    fn events_list(&self, calendar: &str, request: &Request) -> Response {
        if !self.calendars.iter().any(|c| c.id == calendar) {
            return error(404, "notFound");
        }
        let page = request
            .query_value("maxResults")
            .and_then(|n| n.parse().ok())
            .unwrap_or(250usize)
            .max(1);
        let gone = || error(410, "fullSyncRequired");
        let show_deleted = request.query_value("showDeleted").as_deref() == Some("true");
        let valid = |from: u64| from >= self.expired_before;
        let (from, end, offset, deleted) = if let Some(token) = request.query_value("pageToken") {
            match numbers(&token, "ep").as_deref() {
                Some([from, end, offset, deleted]) if *from == 0 || valid(*from) => (
                    *from,
                    *end,
                    usize::try_from(*offset).unwrap_or(0),
                    *deleted == 1,
                ),
                _ => return gone(),
            }
        } else if let Some(token) = request.query_value("syncToken") {
            match numbers(&token, "sync-").as_deref() {
                Some([from]) if valid(*from) => (*from, self.seq, 0, true),
                _ => return gone(),
            }
        } else {
            (0, self.seq, 0, show_deleted)
        };
        let all: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| r.calendar == calendar && r.seq > from && r.seq <= end)
            // A listing from the start has no deletions unless it asks for them.
            .filter(|r| !r.removed || from > 0 || deleted)
            .collect();
        let items: Vec<Value> = all
            .iter()
            .skip(offset)
            .take(page)
            .map(|r| r.event.clone())
            .collect();
        let mut body = json!({"kind": "calendar#events", "summary": calendar, "items": items});
        if offset + page < all.len() {
            body["nextPageToken"] = json!(format!(
                "ep{from}.{end}.{}.{}",
                offset + page,
                u8::from(deleted)
            ));
        } else {
            body["nextSyncToken"] = json!(format!("sync-{end}"));
        }
        Response::json(200, &body)
    }

    /// Answers `rest` (what follows `/calendar/v3`), or `None` when it is not a calendar route.
    pub fn answer(&self, rest: &str, request: &Request) -> Option<Response> {
        let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
        let response = match parts.as_slice() {
            ["users", "me", "calendarList"] => self.list(request),
            ["calendars", calendar, "events"] => {
                self.events_list(&percent_decode(calendar), request)
            }
            ["calendars", calendar, "events", id] => {
                let (calendar, id) = (percent_decode(calendar), percent_decode(id));
                self.rows
                    .iter()
                    .find(|r| r.calendar == calendar && r.id == id)
                    .map_or_else(|| error(404, "notFound"), |r| Response::json(200, &r.event))
            }
            _ => return None,
        };
        Some(match request.method == "GET" {
            true => response,
            false => error(405, "methodNotAllowed"),
        })
    }
}
