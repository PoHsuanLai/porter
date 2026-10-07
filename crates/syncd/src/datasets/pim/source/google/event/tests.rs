use super::*;
use crate::datasets::pim::{PimKind, is_complete};

fn event(json: &str) -> Event {
    serde_json::from_str(json).expect("event")
}

/// The converted lines, unfolded, without the envelope.
fn lines(json: &str) -> Vec<String> {
    let text = to_ics(&event(json)).expect("converted");
    assert!(is_complete(PimKind::Calendar, text.as_bytes()), "{text}");
    assert!(text.ends_with("\r\n"));
    assert!(
        text.split("\r\n").all(|l| l.len() <= 75),
        "a line is folded at 75 octets: {text}"
    );
    text.replace("\r\n ", "")
        .lines()
        .map(str::to_owned)
        .filter(|l| {
            !matches!(
                l.as_str(),
                "BEGIN:VCALENDAR" | "END:VCALENDAR" | "BEGIN:VEVENT" | "END:VEVENT"
            )
        })
        .collect()
}

fn has(lines: &[String], want: &str) -> bool {
    lines.iter().any(|l| l == want)
}

const TIMED: &str = r#"{
  "kind": "calendar#event", "id": "e1", "etag": "\"3181\"", "status": "confirmed",
  "iCalUID": "e1@google.com", "summary": "Dentist, 2nd; visit",
  "description": "Bring the card\nand the forms", "location": "Main St 1",
  "start": {"dateTime": "2026-10-06T10:00:00-07:00", "timeZone": "America/Los_Angeles"},
  "end": {"dateTime": "2026-10-06T11:30:00-07:00", "timeZone": "America/Los_Angeles"},
  "created": "2026-10-01T08:00:00.000Z", "updated": "2026-10-05T12:30:00.123Z",
  "sequence": 2, "transparency": "opaque", "visibility": "private"
}"#;

#[test]
fn each_field_of_a_single_timed_event_is_written_with_the_instant_in_utc() {
    let got = lines(TIMED);
    for want in [
        "UID:e1@google.com",
        "DTSTAMP:20261005T123000Z",
        "CREATED:20261001T080000Z",
        "LAST-MODIFIED:20261005T123000Z",
        "SEQUENCE:2",
        "SUMMARY:Dentist\\, 2nd\\; visit",
        "DTSTART:20261006T170000Z",
        "DTEND:20261006T183000Z",
        "LOCATION:Main St 1",
        "DESCRIPTION:Bring the card\\nand the forms",
        "STATUS:CONFIRMED",
        "CLASS:PRIVATE",
        "TRANSP:OPAQUE",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert!(
        !got.iter()
            .any(|l| l.starts_with("RRULE") || l.contains("TZID"))
    );
}

#[test]
fn an_all_day_event_is_dates_and_the_end_stays_exclusive() {
    let got = lines(
        r#"{"id": "h1", "summary": "Holiday", "start": {"date": "2026-12-24"},
            "end": {"date": "2026-12-26"}, "transparency": "transparent"}"#,
    );
    assert!(has(&got, "DTSTART;VALUE=DATE:20261224"), "{got:#?}");
    assert!(has(&got, "DTEND;VALUE=DATE:20261226"));
    assert!(has(&got, "TRANSP:TRANSPARENT"));
    assert!(has(&got, "UID:h1"), "no iCalUID: the id is the UID");
    assert!(
        has(&got, "DTSTAMP:19700101T000000Z"),
        "no stamp: a fixed one"
    );
}

#[test]
fn a_series_master_keeps_its_zone_and_passes_its_rules_through() {
    let got = lines(
        r#"{"id": "s1", "iCalUID": "s1@google.com", "summary": "Standup",
            "start": {"dateTime": "2026-10-05T09:00:00-07:00", "timeZone": "America/Los_Angeles"},
            "end": {"dateTime": "2026-10-05T09:15:00-07:00", "timeZone": "America/Los_Angeles"},
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T235959Z",
                           "EXDATE;TZID=America/Los_Angeles:20261012T090000",
                           "X-EVIL:injected", "RRULE:FREQ=DAILY\r\nEND:VEVENT"]}"#,
    );
    for want in [
        "DTSTART;TZID=America/Los_Angeles:20261005T090000",
        "DTEND;TZID=America/Los_Angeles:20261005T091500",
        "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T235959Z",
        "EXDATE;TZID=America/Los_Angeles:20261012T090000",
        "RRULE:FREQ=DAILYEND:VEVENT",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert!(!got.iter().any(|l| l.starts_with("X-EVIL")), "{got:#?}");
    assert_eq!(got.iter().filter(|l| *l == "END:VEVENT").count(), 0);
}

#[test]
fn an_exception_has_its_series_uid_and_a_recurrence_id_in_the_masters_form() {
    let got = lines(
        r#"{"id": "s1_20261012T160000Z", "iCalUID": "s1@google.com",
            "recurringEventId": "s1", "summary": "Standup (moved)",
            "originalStartTime": {"dateTime": "2026-10-12T09:00:00-07:00", "timeZone": "America/Los_Angeles"},
            "start": {"dateTime": "2026-10-12T10:00:00-07:00", "timeZone": "America/Los_Angeles"},
            "end": {"dateTime": "2026-10-12T10:15:00-07:00", "timeZone": "America/Los_Angeles"}}"#,
    );
    for want in [
        "UID:s1@google.com",
        "RECURRENCE-ID;TZID=America/Los_Angeles:20261012T090000",
        "DTSTART;TZID=America/Los_Angeles:20261012T100000",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    let all_day = lines(
        r#"{"id": "d_20261012", "iCalUID": "d@google.com", "recurringEventId": "d",
            "originalStartTime": {"date": "2026-10-12"},
            "start": {"date": "2026-10-13"}, "end": {"date": "2026-10-14"}}"#,
    );
    assert!(
        has(&all_day, "RECURRENCE-ID;VALUE=DATE:20261012"),
        "{all_day:#?}"
    );
}

#[test]
fn utc_zone_names_are_written_as_utc_even_in_a_series() {
    for zone in ["UTC", "Etc/UTC", "GMT"] {
        let got = lines(&format!(
            r#"{{"id": "u", "recurrence": ["RRULE:FREQ=DAILY"],
                "start": {{"dateTime": "2026-10-05T09:00:00Z", "timeZone": "{zone}"}}}}"#
        ));
        assert!(has(&got, "DTSTART:20261005T090000Z"), "{zone}: {got:#?}");
    }
}

#[test]
fn people_on_an_event_are_written_with_their_role_and_answer() {
    let got = lines(
        r#"{"id": "m1", "summary": "Review",
            "start": {"dateTime": "2026-10-07T13:00:00Z"}, "end": {"dateTime": "2026-10-07T14:00:00Z"},
            "organizer": {"email": "ada@example.test", "displayName": "Ada, Countess"},
            "attendees": [
              {"email": "grace@example.test", "displayName": "Grace", "responseStatus": "accepted"},
              {"email": "linus@example.test", "responseStatus": "declined", "optional": true},
              {"email": "room@example.test", "displayName": "Room 4", "resource": true, "responseStatus": "tentative"},
              {"email": "no way@example.test"},
              {"displayName": "No address"},
              {"email": "new@example.test"}]}"#,
    );
    for want in [
        "ORGANIZER;CN=\"Ada, Countess\":mailto:ada@example.test",
        "ATTENDEE;CN=Grace;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:grace@example.test",
        "ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=DECLINED:mailto:linus@example.test",
        "ATTENDEE;CN=Room 4;CUTYPE=RESOURCE;ROLE=NON-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:room@example.test",
        "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION:mailto:new@example.test",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert_eq!(got.iter().filter(|l| l.starts_with("ATTENDEE")).count(), 4);
}

#[test]
fn the_status_and_a_long_text_are_carried() {
    let tentative = lines(
        r#"{"id": "t", "status": "tentative", "start": {"dateTime": "2026-10-07T13:00:00Z"}}"#,
    );
    assert!(has(&tentative, "STATUS:TENTATIVE"));
    let cancelled = event(r#"{"id": "c", "status": "cancelled"}"#);
    assert!(cancelled.is_cancelled() && !event(TIMED).is_cancelled());
    let long = format!(
        r#"{{"id": "l", "summary": "{}é{}", "start": {{"date": "2026-10-07"}}}}"#,
        "a".repeat(74),
        "b".repeat(80)
    );
    let text = to_ics(&event(&long)).expect("converted");
    assert!(text.contains("\r\n "), "folded");
    let unfolded = text.replace("\r\n ", "");
    assert!(unfolded.contains(&format!("SUMMARY:{}é{}", "a".repeat(74), "b".repeat(80))));
}

#[test]
fn an_event_that_cannot_be_written_says_why() {
    for (json, want) in [
        (r#"{"id": "x"}"#, ConvertError::NoStart),
        (r#"{"id": "x", "start": {}}"#, ConvertError::NoStart),
        (
            r#"{"id": "x", "start": {"dateTime": "tomorrow"}}"#,
            ConvertError::BadTime,
        ),
        (
            r#"{"id": "x", "start": {"date": "2026-99-99"}}"#,
            ConvertError::BadTime,
        ),
        (
            r#"{"id": "x", "start": {"date": "2026-10-07"}, "end": {"dateTime": "?"}}"#,
            ConvertError::BadTime,
        ),
    ] {
        assert_eq!(to_ics(&event(json)), Err(want), "{json}");
    }
}
