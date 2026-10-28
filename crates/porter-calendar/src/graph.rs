//! Microsoft Graph's calendars (`/me/calendars`), whose events are JSON.
//!
//! Graph gives an event it creates an `iCalUId` of its own, so an event put here also carries its
//! UID in a single-value extended property, and is found by that, or by `iCalUId` for one that
//! came to the calendar another way (an invitation Exchange took in keeps the sender's UID).

use crate::CalendarStore;
use crate::error::CalendarError;
use crate::event::{CalendarRef, Event, Stored, When};
use crate::json::{bare, body, encode, exchange, if_match, removed, request, text, url};
use porter_http::Http;
use porter_http::http::Method;
use porter_ids::WebUrl;
use serde_json::{Value, json};

/// Graph's v1.0 endpoint.
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

/// The extended property an event put here carries its UID in: a string property under a GUID
/// that names porter's events.
const UID_PROPERTY: &str = "String {6f3c1c0e-5d2a-4c1f-9a7e-2b8f0d4e6a51} Name porterUid";

/// A Microsoft account's calendars, through Graph.
#[derive(Debug, Clone)]
pub struct Graph<H> {
    http: H,
    base: String,
}

impl<H: Http> Graph<H> {
    /// Graph at its own endpoint.
    pub fn new(http: H) -> Self {
        Self::at(http, GRAPH)
    }

    /// Graph at `base` (a national cloud's, or a test server's), with no trailing slash.
    pub fn at(http: H, base: &str) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    fn url(&self, path: &str) -> Result<WebUrl, CalendarError> {
        url(&format!("{}{path}", self.base))
    }

    /// The first event in `calendar` matching the OData `filter`.
    async fn first(&self, calendar: &str, filter: &str) -> Result<Option<Value>, CalendarError> {
        let at = self.url(&format!(
            "/me/calendars/{}/events?$filter={}&$top=1",
            encode(calendar),
            encode(filter)
        ))?;
        let response = exchange(&self.http, bare(Method::GET, &at)?, "a search of events").await?;
        Ok(body(&response)?
            .get("value")
            .and_then(Value::as_array)
            .and_then(|found| found.first().cloned()))
    }
}

/// `text` as an OData string literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// `event` as Graph writes one, in UTC.
fn graph_event(event: &Event) -> Value {
    let (start, end, all_day) = match event.when {
        When::AllDay { first, last } => (first.midnight(), last.next().midnight(), true),
        When::Timed { start, end } => (start, end, false),
    };
    let mut out = json!({
        "subject": event.summary,
        "start": { "dateTime": start.local_utc(), "timeZone": "UTC" },
        "end": { "dateTime": end.local_utc(), "timeZone": "UTC" },
        "isAllDay": all_day,
        "body": {
            "contentType": "text",
            "content": event.description.clone().unwrap_or_default(),
        },
        "singleValueExtendedProperties": [{ "id": UID_PROPERTY, "value": event.uid }],
    });
    if let Some(location) = &event.location {
        out["location"] = json!({ "displayName": location });
    }
    out
}

/// What Graph answered for an event, as stored.
fn stored(calendar: &str, uid: &str, event: &Value) -> Result<Stored, CalendarError> {
    let id = text(event, "id")
        .ok_or_else(|| CalendarError::Malformed("an event with no id".to_owned()))?;
    Ok(Stored {
        calendar: calendar.to_owned(),
        uid: uid.to_owned(),
        id: id.to_owned(),
        version: text(event, "@odata.etag")
            .or_else(|| text(event, "changeKey"))
            .map(str::to_owned),
        ics: None,
    })
}

impl<H: Http> CalendarStore for Graph<H> {
    async fn calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let at = self.url("/me/calendars?$select=id,name,hexColor,canEdit")?;
        let response =
            exchange(&self.http, bare(Method::GET, &at)?, "the list of calendars").await?;
        let listed = body(&response)?;
        Ok(listed
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|calendar| {
                Some(CalendarRef {
                    id: text(calendar, "id")?.to_owned(),
                    name: text(calendar, "name").unwrap_or_default().to_owned(),
                    color: text(calendar, "hexColor")
                        .filter(|color| !color.is_empty())
                        .map(str::to_owned),
                    writable: calendar
                        .get("canEdit")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                })
            })
            .collect())
    }

    async fn find(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        let ours = format!(
            "singleValueExtendedProperties/Any(ep: ep/id eq {} and ep/value eq {})",
            literal(UID_PROPERTY),
            literal(uid)
        );
        let found = match self.first(calendar, &ours).await? {
            Some(found) => Some(found),
            None => {
                self.first(calendar, &format!("iCalUId eq {}", literal(uid)))
                    .await?
            }
        };
        found.map(|event| stored(calendar, uid, &event)).transpose()
    }

    async fn put(&self, calendar: &str, event: &Event) -> Result<Stored, CalendarError> {
        let written = graph_event(event);
        let response = match self.find(calendar, &event.uid).await? {
            Some(held) => {
                let mut asked = request(
                    Method::PATCH,
                    &self.url(&format!("/me/events/{}", encode(&held.id)))?,
                    &written,
                )?;
                if let Some(etag) = &held.version {
                    asked = if_match(asked, etag)?;
                }
                exchange(&self.http, asked, "the update of an event").await?
            }
            None => {
                let at = self.url(&format!("/me/calendars/{}/events", encode(calendar)))?;
                exchange(
                    &self.http,
                    request(Method::POST, &at, &written)?,
                    "the creation of an event",
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
        let at = self.url(&format!("/me/events/{}", encode(&held.id)))?;
        let response = self.http.send(bare(Method::DELETE, &at)?).await?;
        removed(&response, "the deletion of an event")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Date, Instant};

    fn event(when: When) -> Event {
        Event {
            uid: "u-1@example.test".to_owned(),
            ics: String::new(),
            summary: "Review".to_owned(),
            when,
            location: Some("Room 4".to_owned()),
            description: None,
        }
    }

    #[test]
    fn an_event_is_written_in_utc_with_its_uid_beside_it() {
        let day = |d| Date {
            year: 2026,
            month: 3,
            day: d,
        };
        let cases = [
            (
                When::Timed {
                    start: Instant(1_774_000_800),
                    end: Instant(1_774_004_400),
                },
                ("2026-03-20T10:00:00", "2026-03-20T11:00:00", false),
            ),
            (
                When::AllDay {
                    first: day(20),
                    last: day(21),
                },
                ("2026-03-20T00:00:00", "2026-03-22T00:00:00", true),
            ),
        ];
        for (when, (start, end, all_day)) in cases {
            let written = graph_event(&event(when));
            assert_eq!(written["start"]["dateTime"], start, "{when:?}");
            assert_eq!(written["end"]["dateTime"], end, "{when:?}");
            assert_eq!(written["isAllDay"], all_day, "{when:?}");
            assert_eq!(written["location"]["displayName"], "Room 4");
            assert_eq!(
                written["singleValueExtendedProperties"][0]["value"],
                "u-1@example.test"
            );
        }
    }

    #[test]
    fn a_quote_in_a_literal_is_doubled() {
        assert_eq!(literal("o'brien"), "'o''brien'");
    }
}
