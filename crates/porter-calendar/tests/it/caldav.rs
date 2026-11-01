//! CalDAV against the fake DAV server: the calendar listed, an event put, found, replaced and
//! removed by its UID, and one stored under another name found by a calendar-query.

use crate::common::{Client, Server};
use porter_calendar::porter_http::http::{Method, header::CONTENT_TYPE};
use porter_calendar::porter_http::{Http, request};
use porter_calendar::{CalDav, CalendarError, CalendarStore, Event, Instant, When};
use porter_ids::WebUrl;

fn ics(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\nUID:{uid}\r\n\
         DTSTAMP:20260320T090000Z\r\nDTSTART:20260320T100000Z\r\nDTEND:20260320T110000Z\r\n\
         SUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn event(uid: &str, summary: &str) -> Event {
    Event {
        uid: uid.to_owned(),
        ics: ics(uid, summary),
        summary: summary.to_owned(),
        when: When::Timed {
            start: Instant(1_774_000_800),
            end: Instant(1_774_004_400),
        },
        location: None,
        description: None,
    }
}

/// Put an event into the fake under a name of the server's own, as an invitation it took in.
async fn put_elsewhere(client: &Client, at: &str, uid: &str, summary: &str) {
    let made = client
        .send(
            request(Method::PUT, &WebUrl::parse(at).unwrap())
                .header(CONTENT_TYPE, "text/calendar")
                .body(ics(uid, summary).into_bytes())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(made.status().is_success());
}

#[tokio::test]
async fn the_calendar_under_the_home_is_listed() {
    let server = Server::start().await;
    let store = CalDav::new(server.client(), server.home.clone());
    let calendars = store.calendars().await.unwrap();
    let ids: Vec<&str> = calendars.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [server.calendar.as_str()],
        "files and contacts are not calendars"
    );
    assert_eq!(calendars[0].name, "calendar");
}

#[tokio::test]
async fn an_event_is_put_found_replaced_and_removed_by_its_uid() {
    let server = Server::start().await;
    let store = CalDav::new(server.client(), server.home.clone());
    let calendar = server.calendar.as_str();
    let uid = "meet 1@example.test";
    assert_eq!(store.find(calendar, uid).await.unwrap(), None);

    let put = store.put(calendar, &event(uid, "Review")).await.unwrap();
    assert!(
        put.id
            .ends_with("/dav/calendar/meet%201%40example.test.ics"),
        "{}",
        put.id
    );
    let found = store.find(calendar, uid).await.unwrap().expect("found");
    assert_eq!(found.id, put.id);
    assert!(found.ics.unwrap().contains("SUMMARY:Review"));

    let again = store
        .put(calendar, &event(uid, "Review, moved"))
        .await
        .unwrap();
    assert_eq!(again.id, put.id, "replaced where it was");
    let found = store.find(calendar, uid).await.unwrap().expect("found");
    assert!(found.ics.unwrap().contains("SUMMARY:Review, moved"));
    assert_ne!(found.version, put.version, "the etag moved");

    assert!(store.remove(calendar, uid).await.unwrap());
    assert_eq!(store.find(calendar, uid).await.unwrap(), None);
    assert!(
        !store.remove(calendar, uid).await.unwrap(),
        "nothing left to remove"
    );
}

#[tokio::test]
async fn an_event_stored_under_another_name_is_found_and_replaced_there() {
    let server = Server::start().await;
    let client = server.client();
    let other = format!("{}from-server.ics", server.calendar);
    put_elsewhere(&client, &other, "invite-7@example.test", "Invited").await;

    let store = CalDav::new(client, server.home.clone());
    let calendar = server.calendar.as_str();
    let found = store
        .find(calendar, "invite-7@example.test")
        .await
        .unwrap()
        .expect("found by its UID");
    assert_eq!(found.id, other);
    assert!(found.version.is_some());

    let put = store
        .put(calendar, &event("invite-7@example.test", "Accepted"))
        .await
        .unwrap();
    assert_eq!(put.id, other, "not a second copy");
    assert_eq!(
        store.find(calendar, "nobody@example.test").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_server_with_no_calendar_home_says_so() {
    let server = Server::start().await;
    let got = CalDav::discover(server.client(), server.home.clone()).await;
    assert!(
        matches!(got, Err(CalendarError::Malformed(_))),
        "the plain fake names no calendar home: {got:?}"
    );
}

#[tokio::test]
async fn a_longer_uid_that_contains_the_one_asked_for_is_another_event() {
    let server = Server::start().await;
    let client = server.client();
    let other = format!("{}from-server.ics", server.calendar);
    put_elsewhere(&client, &other, "invite-70@example.test", "Someone else's").await;

    let store = CalDav::new(client, server.home.clone());
    let calendar = server.calendar.as_str();
    assert_eq!(
        store.find(calendar, "invite-7").await.unwrap(),
        None,
        "a substring match is not the event"
    );
    assert!(!store.remove(calendar, "invite-7").await.unwrap());
    let put = store
        .put(calendar, &event("invite-7", "Mine"))
        .await
        .unwrap();
    assert_ne!(put.id, other, "the other event is not overwritten");
    let theirs = store
        .find(calendar, "invite-70@example.test")
        .await
        .unwrap();
    assert!(theirs.unwrap().ics.unwrap().contains("Someone else's"));
}
