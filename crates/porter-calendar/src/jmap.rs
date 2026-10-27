//! JMAP for Calendars (`urn:ietf:params:jmap:calendars`), with events as JSCalendar (RFC 8984).
//!
//! The session is the caller's: [`Jmap::new`] takes the API URL and the account the session
//! named for calendars. Each method here is one JMAP request, its calls chained by result
//! references where one needs another's answer.

use crate::CalendarStore;
use crate::error::CalendarError;
use crate::event::{CalendarRef, Event, Stored, When};
use crate::json::{body, exchange, request, text};
use porter_core::WebUrl;
use porter_http::{Http, Method};
use serde_json::{Value, json};

/// The capabilities every request names it uses.
const USING: [&str; 2] = [
    "urn:ietf:params:jmap:core",
    "urn:ietf:params:jmap:calendars",
];

/// The id a create names its one event by, so the answer can be read back under it.
const NEW: &str = "e";

/// Which of `CalendarEvent/set`'s three changes a call makes.
#[derive(Debug, Clone, Copy)]
enum Change {
    Create,
    Update,
    Destroy,
}

impl Change {
    /// The members of the answer that hold what was done and what was refused.
    fn members(self) -> (&'static str, &'static str) {
        match self {
            Change::Create => ("created", "notCreated"),
            Change::Update => ("updated", "notUpdated"),
            Change::Destroy => ("destroyed", "notDestroyed"),
        }
    }
}

/// A JMAP account's calendars.
#[derive(Debug, Clone)]
pub struct Jmap<H> {
    http: H,
    api: WebUrl,
    account: String,
}

impl<H: Http> Jmap<H> {
    /// The calendars of `account` through the API at `api`.
    pub fn new(http: H, api: WebUrl, account: impl Into<String>) -> Self {
        Self {
            http,
            api,
            account: account.into(),
        }
    }

    /// One request of `calls`, each `(method, arguments)` called by its index; the answers in
    /// order. An error answer is the call's failure.
    async fn call(&self, calls: Vec<(&str, Value)>) -> Result<Vec<Value>, CalendarError> {
        let method_calls: Vec<Value> = calls
            .into_iter()
            .enumerate()
            .map(|(at, (name, args))| json!([name, args, at.to_string()]))
            .collect();
        let asked = json!({ "using": USING, "methodCalls": method_calls });
        let response = exchange(
            &self.http,
            request(Method::Post, self.api.clone(), &asked),
            "a JMAP request",
        )
        .await?;
        let answered = body(&response)?;
        let answers = answered
            .get("methodResponses")
            .and_then(Value::as_array)
            .ok_or_else(|| CalendarError::Malformed("no methodResponses".to_owned()))?;
        answers
            .iter()
            .map(|answer| {
                let name = answer.get(0).and_then(Value::as_str).unwrap_or_default();
                let args = answer.get(1).cloned().unwrap_or(Value::Null);
                if name == "error" {
                    let kind = text(&args, "type").unwrap_or("unknown");
                    return Err(match kind {
                        "forbidden" | "accountNotFound" => CalendarError::Unauthorized,
                        other => CalendarError::Malformed(format!("JMAP error {other}")),
                    });
                }
                Ok(args)
            })
            .collect()
    }

    /// The JMAP id of the event with `uid` in `calendar`.
    async fn event_id(&self, calendar: &str, uid: &str) -> Result<Option<String>, CalendarError> {
        let answers = self
            .call(vec![(
                "CalendarEvent/query",
                json!({
                    "accountId": self.account,
                    "filter": { "inCalendars": [calendar], "uid": uid },
                    "limit": 1,
                }),
            )])
            .await?;
        Ok(answers
            .first()
            .and_then(|answer| answer.get("ids"))
            .and_then(Value::as_array)
            .and_then(|ids| ids.first())
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// A `CalendarEvent/set` of `args`, and what it said of `id`: `Ok(Some(answer))` for one
    /// made, changed or destroyed, `Ok(None)` for one not found.
    async fn set(
        &self,
        mut args: Value,
        change: Change,
        id: &str,
    ) -> Result<Option<Value>, CalendarError> {
        args["accountId"] = json!(self.account);
        let answers = self.call(vec![("CalendarEvent/set", args)]).await?;
        let answer = answers
            .into_iter()
            .next()
            .ok_or_else(|| CalendarError::Malformed("no answer to CalendarEvent/set".to_owned()))?;
        let (done, refused) = change.members();
        if let Some(error) = answer.get(refused).and_then(|r| r.get(id)) {
            return match text(error, "type") {
                Some("notFound") => Ok(None),
                Some("forbidden") => Err(CalendarError::Unauthorized),
                other => Err(CalendarError::Malformed(format!(
                    "CalendarEvent/set refused: {}",
                    other.unwrap_or("unknown")
                ))),
            };
        }
        Ok(Some(match change {
            Change::Destroy => Value::Null,
            _ => answer
                .get(done)
                .and_then(|made| made.get(id))
                .cloned()
                .unwrap_or(Value::Null),
        }))
    }
}

/// `event` as JSCalendar: a start in `Etc/UTC` and a duration; an all-day event as whole days,
/// shown without a time.
fn jscalendar(calendar: &str, event: &Event) -> Value {
    let (start, duration, all_day) = match event.when {
        When::AllDay { first, last } => (
            first.midnight(),
            format!("P{}D", first.days_until(last) + 1),
            true,
        ),
        When::Timed { start, end } => (start, format!("PT{}S", (end.0 - start.0).max(0)), false),
    };
    let mut out = json!({
        "@type": "Event",
        "calendarIds": { (calendar): true },
        "uid": event.uid,
        "title": event.summary,
        "start": start.local_utc(),
        "timeZone": "Etc/UTC",
        "duration": duration,
        "showWithoutTime": all_day,
    });
    if let Some(description) = &event.description {
        out["description"] = json!(description);
    }
    if let Some(location) = &event.location {
        out["locations"] = json!({ "l1": { "@type": "Location", "name": location } });
    }
    out
}

impl<H: Http> CalendarStore for Jmap<H> {
    async fn calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        let answers = self
            .call(vec![(
                "Calendar/get",
                json!({ "accountId": self.account, "ids": null }),
            )])
            .await?;
        Ok(answers
            .first()
            .and_then(|answer| answer.get("list"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|calendar| {
                let rights = calendar.get("myRights");
                let may = |right: &str| rights.and_then(|r| r.get(right)).and_then(Value::as_bool);
                Some(CalendarRef {
                    id: text(calendar, "id")?.to_owned(),
                    name: text(calendar, "name").unwrap_or_default().to_owned(),
                    color: text(calendar, "color").map(str::to_owned),
                    writable: may("mayAddItems")
                        .or_else(|| may("mayWriteAll"))
                        .unwrap_or(true),
                })
            })
            .collect())
    }

    async fn find(&self, calendar: &str, uid: &str) -> Result<Option<Stored>, CalendarError> {
        Ok(self.event_id(calendar, uid).await?.map(|id| Stored {
            calendar: calendar.to_owned(),
            uid: uid.to_owned(),
            id,
            version: None,
            ics: None,
        }))
    }

    async fn put(&self, calendar: &str, event: &Event) -> Result<Stored, CalendarError> {
        let written = jscalendar(calendar, event);
        let id = match self.event_id(calendar, &event.uid).await? {
            Some(id) => {
                self.set(
                    json!({ "update": { (id.as_str()): written } }),
                    Change::Update,
                    &id,
                )
                .await?
                .ok_or(CalendarError::Changed)?;
                id
            }
            None => {
                let made = self
                    .set(json!({ "create": { (NEW): written } }), Change::Create, NEW)
                    .await?
                    .ok_or(CalendarError::Changed)?;
                text(&made, "id")
                    .ok_or_else(|| {
                        CalendarError::Malformed("a created event with no id".to_owned())
                    })?
                    .to_owned()
            }
        };
        Ok(Stored {
            calendar: calendar.to_owned(),
            uid: event.uid.clone(),
            id,
            version: None,
            ics: None,
        })
    }

    async fn remove(&self, calendar: &str, uid: &str) -> Result<bool, CalendarError> {
        let Some(id) = self.event_id(calendar, uid).await? else {
            return Ok(false);
        };
        Ok(self
            .set(json!({ "destroy": [&id] }), Change::Destroy, &id)
            .await?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Date, Instant};

    #[test]
    fn an_event_is_a_start_and_a_duration() {
        let day = |d| Date {
            year: 2026,
            month: 3,
            day: d,
        };
        let cases = [
            (
                When::AllDay {
                    first: day(20),
                    last: day(22),
                },
                ("2026-03-20T00:00:00", "P3D", true),
            ),
            (
                When::Timed {
                    start: Instant(1_774_000_800),
                    end: Instant(1_774_004_400),
                },
                ("2026-03-20T10:00:00", "PT3600S", false),
            ),
        ];
        for (when, (start, duration, all_day)) in cases {
            let written = jscalendar(
                "cal-1",
                &Event {
                    uid: "u-3@example.test".to_owned(),
                    ics: String::new(),
                    summary: "Walk".to_owned(),
                    when,
                    location: Some("Park".to_owned()),
                    description: None,
                },
            );
            assert_eq!(written["start"], start, "{when:?}");
            assert_eq!(written["duration"], duration, "{when:?}");
            assert_eq!(written["showWithoutTime"], all_day, "{when:?}");
            assert_eq!(written["calendarIds"]["cal-1"], true);
            assert_eq!(written["locations"]["l1"]["name"], "Park");
        }
    }
}
