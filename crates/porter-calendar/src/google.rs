//! The Google Calendar API (v3), whose events are JSON.
//!
//! A new event goes in through `events/import`, which keeps the event's own `iCalUID` (an
//! `insert` would give it Google's), so an event is found again by its UID with the `iCalUID`
//! query every event list takes.

use crate::CalendarStore;
use crate::error::CalendarError;
use crate::event::{CalendarRef, Event, Stored, When};
use crate::json::{bare, body, encode, exchange, request, text, url};
use porter_core::WebUrl;
use porter_http::{Http, Method};
use serde_json::{Value, json};

/// The Calendar API's endpoint.
pub const GOOGLE: &str = "https://www.googleapis.com/calendar/v3";

/// A Google account's calendars.
#[derive(Debug, Clone)]
pub struct Google<H> {
    http: H,
    base: String,
}

impl<H: Http> Google<H> {
    /// The Calendar API at its own endpoint.
    pub fn new(http: H) -> Self {
        Self::at(http, GOOGLE)
    }

    /// The Calendar API at `base` (a test server's), with no trailing slash.
    pub fn at(http: H, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    fn url(&self, path: &str) -> Result<WebUrl, CalendarError> {
        url(&format!("{}{path}", self.base))
    }

    fn events(&self, calendar: &str, rest: &str) -> Result<WebUrl, CalendarError> {
        self.url(&format!("/calendars/{}/events{rest}", encode(calendar)))
    }
}

/// `event` as the Calendar API writes one: an all-day event by its dates, the last one past it
/// (the API's end is exclusive), a timed one in UTC.
fn google_event(event: &Event) -> Value {
    let (start, end) = match event.when {
        When::AllDay { first, last } => (
            json!({ "date": first.to_string() }),
            json!({ "date": last.next().to_string() }),
        ),
        When::Timed { start, end } => (
            json!({ "dateTime": start.rfc3339() }),
            json!({ "dateTime": end.rfc3339() }),
        ),
    };
    let mut out = json!({
        "iCalUID": event.uid,
        "summary": event.summary,
        "start": start,
        "end": end,
    });
    if let Some(location) = &event.location {
        out["location"] = json!(location);
    }
    if let Some(description) = &event.description {
        out["description"] = json!(description);
    }
    out
}

fn stored(calendar: &str, uid: &str, event: &Value) -> Result<Stored, CalendarError> {
    let id = text(event, "id")
        .ok_or_else(|| CalendarError::Malformed("an event with no id".to_owned()))?;
    Ok(Stored {
        calendar: calendar.to_owned(),
        uid: uid.to_owned(),
        id: id.to_owned(),
        version: text(event, "etag").map(str::to_owned),
        ics: None,
    })
}

impl<H: Http> CalendarStore for Google<H> {
    async fn calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let at = self.url("/users/me/calendarList")?;
        let response = exchange(&self.http, bare(Method::Get, at), "the list of calendars").await?;
        let listed = body(&response)?;
        Ok(listed
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|calendar| {
                Some(CalendarRef {
                    id: text(calendar, "id")?.to_owned(),
                    name: text(calendar, "summaryOverride")
                        .or_else(|| text(calendar, "summary"))
                        .unwrap_or_default()
                        .to_owned(),
                    color: text(calendar, "backgroundColor").map(str::to_owned),
                    writable: matches!(
                        text(calendar, "accessRole"),
                        None | Some("owner" | "writer")
                    ),
                })
            })
            .collect())
    }

    async fn find(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        let at = self.events(calendar, &format!("?iCalUID={}", encode(uid)))?;
        let response = self.http.send(bare(Method::Get, at)).await?;
        if response.status.0 == 404 {
            return Ok(None);
        }
        if !response.status.is_success() {
            return Err(CalendarError::of_status(
                response.status.0,
                "a search of events",
            ));
        }
        body(&response)?
            .get("items")
            .and_then(Value::as_array)
            .and_then(|found| {
                found
                    .iter()
                    .find(|event| text(event, "status") != Some("cancelled"))
            })
            .map(|event| stored(calendar, uid, event))
            .transpose()
    }

    async fn put(&self, calendar: &str, event: &Event) -> Result<Stored, CalendarError> {
        let written = google_event(event);
        let response = match self.find(calendar, &event.uid).await? {
            Some(held) => {
                let mut asked = request(
                    Method::Put,
                    self.events(calendar, &format!("/{}", encode(&held.id)))?,
                    &written,
                );
                if let Some(etag) = &held.version {
                    asked = asked.with_header("If-Match", etag.clone());
                }
                exchange(&self.http, asked, "the update of an event").await?
            }
            None => {
                let at = self.events(calendar, "/import")?;
                exchange(
                    &self.http,
                    request(Method::Post, at, &written),
                    "the import of an event",
                )
                .await?
            }
        };
        stored(calendar, &event.uid, &body(&response)?)
    }

    async fn remove(&self, calendar: &str, uid: &str) -> Result<bool, CalendarError> {
        let Some(held) = self.find(calendar, uid).await? else {
            return Ok(false);
        };
        let at = self.events(calendar, &format!("/{}", encode(&held.id)))?;
        let response = self.http.send(bare(Method::Delete, at)).await?;
        match response.status.0 {
            200..=299 => Ok(true),
            404 | 410 => Ok(false),
            status => Err(CalendarError::of_status(status, "the deletion of an event")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Date, Instant};

    #[test]
    fn an_event_keeps_its_uid_and_an_all_day_one_ends_the_day_after() {
        let day = |d| Date {
            year: 2026,
            month: 12,
            day: d,
        };
        let cases = [
            (
                When::AllDay {
                    first: day(30),
                    last: day(31),
                },
                (json!({"date": "2026-12-30"}), json!({"date": "2027-01-01"})),
            ),
            (
                When::Timed {
                    start: Instant(1_774_000_800),
                    end: Instant(1_774_004_400),
                },
                (
                    json!({"dateTime": "2026-03-20T10:00:00Z"}),
                    json!({"dateTime": "2026-03-20T11:00:00Z"}),
                ),
            ),
        ];
        for (when, (start, end)) in cases {
            let written = google_event(&Event {
                uid: "u-2@example.test".to_owned(),
                ics: String::new(),
                summary: "Party".to_owned(),
                when,
                location: None,
                description: Some("Bring cake.".to_owned()),
            });
            assert_eq!(written["iCalUID"], "u-2@example.test");
            assert_eq!(written["start"], start, "{when:?}");
            assert_eq!(written["end"], end, "{when:?}");
            assert_eq!(written["description"], "Bring cake.");
            assert!(written.get("location").is_none());
        }
    }
}
