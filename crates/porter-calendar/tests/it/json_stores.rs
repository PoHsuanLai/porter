//! Graph, Google and JMAP against a scripted seam: what each sends to find, make, replace and
//! remove an event, and what it makes of the answers.

use crate::common::Script;
use porter_calendar::{CalendarError, CalendarStore, Date, Event, Google, Graph, Jmap, When};
use porter_ids::WebUrl;

fn event(uid: &str) -> Event {
    let day = Date {
        year: 2026,
        month: 3,
        day: 20,
    };
    Event {
        uid: uid.to_owned(),
        ics: String::new(),
        summary: "Offsite".to_owned(),
        when: When::AllDay {
            first: day,
            last: day,
        },
        location: None,
        description: None,
    }
}

#[tokio::test]
async fn graph_makes_an_event_it_cannot_find_and_patches_one_it_can() {
    let script = Script::new(&[
        // put: not found by our property, nor by iCalUId, so created.
        (200, r#"{"value":[]}"#),
        (200, r#"{"value":[]}"#),
        (201, r#"{"id":"AAk=","@odata.etag":"W/\"1\""}"#),
        // put again: found by our property, so patched with its etag.
        (200, r#"{"value":[{"id":"AAk=","@odata.etag":"W/\"1\""}]}"#),
        (200, r#"{"id":"AAk=","@odata.etag":"W/\"2\""}"#),
        // remove: found, deleted.
        (200, r#"{"value":[{"id":"AAk="}]}"#),
        (204, ""),
    ]);
    let store = Graph::at(&script, "https://graph.test/v1.0");
    let made = store.put("cal-1", &event("u-1")).await.unwrap();
    assert_eq!(made.id, "AAk=");
    assert_eq!(made.version.as_deref(), Some("W/\"1\""));
    let changed = store.put("cal-1", &event("u-1")).await.unwrap();
    assert_eq!(changed.version.as_deref(), Some("W/\"2\""));
    assert!(store.remove("cal-1", "u-1").await.unwrap());

    let lines = script.lines();
    assert!(lines[0].starts_with("GET https://graph.test/v1.0/me/calendars/cal-1/events?$filter=singleValueExtendedProperties"), "{}", lines[0]);
    assert!(
        lines[1].contains("iCalUId%20eq%20%27u-1%27"),
        "{}",
        lines[1]
    );
    assert_eq!(
        lines[2],
        "POST https://graph.test/v1.0/me/calendars/cal-1/events"
    );
    assert_eq!(script.body(2)["isAllDay"], true);
    assert_eq!(script.body(2)["end"]["dateTime"], "2026-03-21T00:00:00");
    assert_eq!(lines[4], "PATCH https://graph.test/v1.0/me/events/AAk%3D");
    assert_eq!(script.header(4, "If-Match").as_deref(), Some("W/\"1\""));
    assert_eq!(lines[6], "DELETE https://graph.test/v1.0/me/events/AAk%3D");
}

#[tokio::test]
async fn google_imports_a_new_event_and_puts_over_a_held_one() {
    let script = Script::new(&[
        (200, r#"{"items":[]}"#),
        (200, r#"{"id":"ev1","etag":"\"e1\""}"#),
        (200, r#"{"items":[{"id":"ev1","etag":"\"e1\""}]}"#),
        (200, r#"{"id":"ev1","etag":"\"e2\""}"#),
        (200, r#"{"items":[{"id":"ev1","status":"cancelled"}]}"#),
    ]);
    let store = Google::at(&script, "https://google.test/calendar/v3");
    let made = store.put("me@example.test", &event("u-2")).await.unwrap();
    assert_eq!(made.id, "ev1");
    let changed = store.put("me@example.test", &event("u-2")).await.unwrap();
    assert_eq!(changed.version.as_deref(), Some("\"e2\""));
    assert!(
        !store.remove("me@example.test", "u-2").await.unwrap(),
        "a cancelled event is not there to remove"
    );

    let lines = script.lines();
    assert_eq!(
        lines[0],
        "GET https://google.test/calendar/v3/calendars/me%40example.test/events?iCalUID=u-2"
    );
    assert_eq!(
        lines[1],
        "POST https://google.test/calendar/v3/calendars/me%40example.test/events/import"
    );
    assert_eq!(script.body(1)["iCalUID"], "u-2");
    assert_eq!(script.body(1)["end"]["date"], "2026-03-21");
    assert_eq!(
        lines[3],
        "PUT https://google.test/calendar/v3/calendars/me%40example.test/events/ev1"
    );
    assert_eq!(script.header(3, "If-Match").as_deref(), Some("\"e1\""));
    assert_eq!(lines.len(), 5, "nothing deleted");
}

#[tokio::test]
async fn jmap_creates_by_uid_and_says_when_it_was_refused() {
    let script = Script::new(&[
        (
            200,
            r#"{"methodResponses":[["CalendarEvent/query",{"ids":[]},"0"]]}"#,
        ),
        (
            200,
            r#"{"methodResponses":[["CalendarEvent/set",{"created":{"e":{"id":"J1"}}},"0"]]}"#,
        ),
        (
            200,
            r#"{"methodResponses":[["CalendarEvent/query",{"ids":["J1"]},"0"]]}"#,
        ),
        (
            200,
            r#"{"methodResponses":[["CalendarEvent/set",{"notUpdated":{"J1":{"type":"forbidden"}}},"0"]]}"#,
        ),
        (
            200,
            r#"{"methodResponses":[["error",{"type":"accountNotFound"},"0"]]}"#,
        ),
    ]);
    let api = WebUrl::parse("https://jmap.test/api/").unwrap();
    let store = Jmap::new(&script, api, "A1");
    let made = store.put("cal-9", &event("u-3")).await.unwrap();
    assert_eq!(made.id, "J1");
    let create = script.body(1);
    let args = &create["methodCalls"][0][1];
    assert_eq!(args["accountId"], "A1");
    assert_eq!(args["create"]["e"]["uid"], "u-3");
    assert_eq!(args["create"]["e"]["duration"], "P1D");
    assert_eq!(script.body(0)["methodCalls"][0][1]["filter"]["uid"], "u-3");

    let refused = store.put("cal-9", &event("u-3")).await;
    assert!(
        matches!(refused, Err(CalendarError::Unauthorized)),
        "{refused:?}"
    );
    let gone = store.find("cal-9", "u-3").await;
    assert!(matches!(gone, Err(CalendarError::Unauthorized)), "{gone:?}");
}
