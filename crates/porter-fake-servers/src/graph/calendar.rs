//! The calendar half of the fake Graph: `GET /v1.0/me/calendars`, a calendar's
//! `events/delta` (a `$deltatoken` or `$skiptoken` in the link it hands out, `@removed` stubs for
//! deleted events, `410 syncStateNotFound` for a token the server dropped), and one event by id.
//! Events are kept as the JSON a test gives; the fake adds `id`, `changeKey` and
//! `lastModifiedDateTime`. Behind the same bearer as the drive.

use crate::http::{Request, Response, percent_decode};
use serde_json::{Value, json};

/// One calendar.
#[derive(Debug, Clone)]
struct Calendar {
    id: String,
    name: String,
    hex_color: String,
    color: String,
}

/// One event, or the stub a deletion left.
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
    /// Delta tokens older than this are dropped.
    expired_before: u64,
}

fn error(status: u16, code: &str) -> Response {
    Response::json(status, &json!({"error": {"code": code, "message": code}}))
}

impl Calendars {
    /// Adds the calendar `id`, or renames and recolours it.
    pub fn set_calendar(&mut self, id: &str, name: &str, hex_color: &str, color: &str) {
        let next = Calendar {
            id: id.to_owned(),
            name: name.to_owned(),
            hex_color: hex_color.to_owned(),
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
            map.insert("id".into(), json!(id));
            map.insert("changeKey".into(), json!(format!("ck{}", self.seq)));
            map.entry("lastModifiedDateTime")
                .or_insert_with(|| json!("2026-10-05T12:30:00.0000000Z"));
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

    /// Deletes an event, leaving the `@removed` stub the next delta reports.
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
            row.event = json!({"id": id, "@removed": {"reason": "deleted"}});
        }
    }

    /// Drops every delta token handed out so far.
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
    /// series (and one moved occurrence of it), an all-day event and a cancelled one, and
    /// `cal-work` with one event.
    pub fn seed(&mut self) {
        let when = |date_time: &str, zone: &str| json!({"dateTime": date_time, "timeZone": zone});
        self.set_calendar("cal-personal", "Personal", "#0078d4", "lightBlue");
        self.set_calendar("cal-work", "Work", "", "lightGreen");
        self.put_event(
            "cal-personal",
            "ev-dentist",
            json!({
                "iCalUId": "uid-dentist", "subject": "Dentist",
                "start": when("2026-10-06T10:00:00.0000000", "UTC"),
                "end": when("2026-10-06T11:00:00.0000000", "UTC"),
                "isAllDay": false, "isCancelled": false, "type": "singleInstance",
                "location": {"displayName": "Main St 1"},
                "body": {"contentType": "text", "content": "Bring the card"},
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-standup",
            json!({
                "iCalUId": "uid-standup", "subject": "Standup", "type": "seriesMaster",
                "start": when("2026-10-05T09:00:00.0000000", "Pacific Standard Time"),
                "end": when("2026-10-05T09:15:00.0000000", "Pacific Standard Time"),
                "isAllDay": false, "isCancelled": false,
                "recurrence": {
                    "pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["monday"], "firstDayOfWeek": "sunday"},
                    "range": {"type": "noEnd", "startDate": "2026-10-05"},
                },
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-standup-moved",
            json!({
                "iCalUId": "uid-standup", "subject": "Standup (moved)", "type": "exception",
                "seriesMasterId": "ev-standup", "originalStart": "2026-10-12T16:00:00Z",
                "start": when("2026-10-12T10:00:00.0000000", "Pacific Standard Time"),
                "end": when("2026-10-12T10:15:00.0000000", "Pacific Standard Time"),
                "isAllDay": false, "isCancelled": false,
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-holiday",
            json!({
                "iCalUId": "uid-holiday", "subject": "Holiday", "type": "singleInstance",
                "start": when("2026-12-24T00:00:00.0000000", "UTC"),
                "end": when("2026-12-26T00:00:00.0000000", "UTC"),
                "isAllDay": true, "isCancelled": false,
            }),
        );
        self.put_event(
            "cal-personal",
            "ev-called-off",
            json!({
                "iCalUId": "uid-called-off", "subject": "Called off", "type": "singleInstance",
                "start": when("2026-10-08T15:00:00.0000000", "UTC"),
                "end": when("2026-10-08T16:00:00.0000000", "UTC"),
                "isAllDay": false, "isCancelled": true,
            }),
        );
        self.put_event(
            "cal-work",
            "ev-review",
            json!({
                "iCalUId": "uid-review", "subject": "Design review", "type": "singleInstance",
                "start": when("2026-10-07T13:00:00.0000000", "UTC"),
                "end": when("2026-10-07T14:00:00.0000000", "UTC"),
                "isAllDay": false, "isCancelled": false,
                "organizer": {"emailAddress": {"name": "Ada", "address": "ada@example.test"}},
                "attendees": [{"type": "required", "status": {"response": "accepted"},
                               "emailAddress": {"name": "Grace", "address": "grace@example.test"}}],
            }),
        );
    }
}

/// What a request under `/v1.0/me/calendars` addresses.
enum Route {
    List,
    Delta(String),
    Event(String, String),
    Unknown,
}

fn route(rest: &str) -> Route {
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        return Route::List;
    }
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [calendar, "events", "delta"] => Route::Delta(percent_decode(calendar)),
        [calendar, "events", event] => {
            Route::Event(percent_decode(calendar), percent_decode(event))
        }
        _ => Route::Unknown,
    }
}

fn delta(
    state: &Calendars,
    base: &str,
    page_default: usize,
    calendar: &str,
    request: &Request,
) -> Response {
    if !state.calendars.iter().any(|c| c.id == calendar) {
        return error(404, "ErrorItemNotFound");
    }
    let page = request
        .header("prefer")
        .and_then(|p| {
            p.split(',')
                .find_map(|v| v.trim().strip_prefix("odata.maxpagesize="))
        })
        .and_then(|n| n.parse().ok())
        .unwrap_or(page_default)
        .max(1);
    let valid = |from: u64| from >= state.expired_before;
    let (from, end, offset) = if let Some(token) = request.query_value("$deltatoken") {
        match token.parse::<u64>() {
            Ok(from) if valid(from) => (from, state.seq, 0),
            _ => return error(410, "syncStateNotFound"),
        }
    } else if let Some(skip) = request.query_value("$skiptoken") {
        let parts: Vec<u64> = skip.split('.').filter_map(|p| p.parse().ok()).collect();
        match parts.as_slice() {
            [from, end, offset] if *from == 0 || valid(*from) => {
                (*from, *end, usize::try_from(*offset).unwrap_or(0))
            }
            _ => return error(410, "syncStateNotFound"),
        }
    } else {
        (0, state.seq, 0)
    };
    let all: Vec<&Row> = state
        .rows
        .iter()
        .filter(|r| r.calendar == calendar && r.seq > from && r.seq <= end)
        // The first listing is everything there is, without deletions.
        .filter(|r| from > 0 || !r.removed)
        .collect();
    let items: Vec<Value> = all
        .iter()
        .skip(offset)
        .take(page)
        .map(|r| r.event.clone())
        .collect();
    let target = format!("{base}{}", request.path());
    let mut body = json!({"value": items});
    if offset + page < all.len() {
        body["@odata.nextLink"] = json!(format!(
            "{target}?$skiptoken={from}.{end}.{}",
            offset + page
        ));
    } else {
        body["@odata.deltaLink"] = json!(format!("{target}?$deltatoken={end}"));
    }
    Response::json(200, &body)
}

/// Answers a request for `path` (which starts with `/v1.0/me/calendars`) or `None` when it is
/// not one of the calendar routes.
pub fn answer(
    state: &Calendars,
    base: &str,
    page_default: usize,
    path: &str,
    request: &Request,
) -> Option<Response> {
    let rest = path.strip_prefix("/v1.0/me/calendars")?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    if request.method != "GET" {
        return Some(error(405, "ErrorInvalidMethod"));
    }
    Some(match route(rest) {
        Route::List => {
            let value: Vec<Value> = state
                .calendars
                .iter()
                .map(|c| json!({"id": c.id, "name": c.name, "hexColor": c.hex_color, "color": c.color}))
                .collect();
            Response::json(200, &json!({"value": value}))
        }
        Route::Delta(calendar) => delta(state, base, page_default, &calendar, request),
        Route::Event(calendar, id) => state
            .rows
            .iter()
            .find(|r| r.calendar == calendar && r.id == id && !r.removed)
            .map_or_else(
                || error(404, "ErrorItemNotFound"),
                |r| Response::json(200, &r.event),
            ),
        Route::Unknown => error(404, "ErrorItemNotFound"),
    })
}
