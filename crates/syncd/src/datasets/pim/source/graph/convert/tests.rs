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
  "id": "AAA=", "iCalUId": "uid-1", "subject": "Dentist, 2nd; visit",
  "start": {"dateTime": "2026-10-06T10:00:00.0000000", "timeZone": "UTC"},
  "end": {"dateTime": "2026-10-06T11:30:00.0000000", "timeZone": "UTC"},
  "isAllDay": false, "isCancelled": false,
  "location": {"displayName": "Main St 1"},
  "body": {"contentType": "text", "content": "Bring the card\r\nand the forms"},
  "createdDateTime": "2026-10-01T08:00:00.1234567Z",
  "lastModifiedDateTime": "2026-10-05T12:30:00.9999999Z",
  "type": "singleInstance"
}"#;

#[test]
fn each_field_of_a_timed_event_is_written() {
    let got = lines(TIMED);
    for want in [
        "UID:uid-1",
        "DTSTAMP:20261005T123000Z",
        "CREATED:20261001T080000Z",
        "LAST-MODIFIED:20261005T123000Z",
        "SUMMARY:Dentist\\, 2nd\\; visit",
        "DTSTART:20261006T100000Z",
        "DTEND:20261006T113000Z",
        "LOCATION:Main St 1",
        "DESCRIPTION:Bring the card\\nand the forms",
        "STATUS:CONFIRMED",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert!(!got.iter().any(|l| l.starts_with("RRULE")));
}

#[test]
fn the_time_zone_is_utc_a_windows_name_an_iana_name_or_floating() {
    let with = |zone: &str| {
        lines(&format!(
            r#"{{"id":"1","start":{{"dateTime":"2026-10-06T10:00:00.0000000","timeZone":"{zone}"}},"end":{{"dateTime":"2026-10-06T11:00:00.0000000","timeZone":"{zone}"}}}}"#
        ))
    };
    let cases: [(&str, &str, &str); 3] = [
        ("UTC", "DTSTART:20261006T100000Z", "DTEND:20261006T110000Z"),
        (
            "Pacific Standard Time",
            "DTSTART;TZID=America/Los_Angeles:20261006T100000",
            "DTEND;TZID=America/Los_Angeles:20261006T110000",
        ),
        (
            "Europe/Berlin",
            "DTSTART;TZID=Europe/Berlin:20261006T100000",
            "DTEND;TZID=Europe/Berlin:20261006T110000",
        ),
    ];
    for (zone, start, end) in cases {
        let got = with(zone);
        assert!(has(&got, start) && has(&got, end), "{zone}: {got:#?}");
    }
    let floating = with("Mars Standard Time");
    assert!(has(&floating, "DTSTART:20261006T100000"), "{floating:#?}");
    let comments = floating
        .iter()
        .filter(|l| l.starts_with("COMMENT:"))
        .count();
    assert_eq!(comments, 1, "one comment however many times it is used");
}

#[test]
fn an_all_day_event_has_dates_and_an_exclusive_end() {
    let got = lines(
        r#"{"id":"1","iCalUId":"d","subject":"Holiday","isAllDay":true,
            "start":{"dateTime":"2026-12-24T00:00:00.0000000","timeZone":"UTC"},
            "end":{"dateTime":"2026-12-26T00:00:00.0000000","timeZone":"UTC"}}"#,
    );
    assert!(has(&got, "DTSTART;VALUE=DATE:20261224"), "{got:#?}");
    assert!(has(&got, "DTEND;VALUE=DATE:20261226"), "{got:#?}");
}

#[test]
fn a_status_class_and_transparency_follow_the_event() {
    let with = |extra: &str| {
        lines(&format!(
            r#"{{"id":"1","start":{{"dateTime":"2026-10-06T10:00:00.0000000","timeZone":"UTC"}}{extra}}}"#
        ))
    };
    let cases: [(&str, &str); 4] = [
        (r#","isCancelled":true"#, "STATUS:CANCELLED"),
        (r#","showAs":"tentative""#, "STATUS:TENTATIVE"),
        (r#","sensitivity":"private""#, "CLASS:PRIVATE"),
        (r#","showAs":"free""#, "TRANSP:TRANSPARENT"),
    ];
    for (extra, want) in cases {
        let got = with(extra);
        assert!(has(&got, want), "{extra}: {got:#?}");
    }
    assert!(has(
        &with(r#","isCancelled":true,"showAs":"tentative""#),
        "STATUS:CANCELLED"
    ));
}

#[test]
fn an_html_body_becomes_text_and_an_empty_one_falls_back_to_the_preview() {
    let html = lines(
        r#"{"id":"1","start":{"dateTime":"2026-10-06T10:00:00.0000000"},
            "body":{"contentType":"html","content":"<html><body><p>Agenda &amp; notes</p><p>Line<br>two</p></body></html>"}}"#,
    );
    assert!(
        has(&html, "DESCRIPTION:Agenda & notes\\nLine\\ntwo"),
        "{html:#?}"
    );
    let preview = lines(
        r#"{"id":"1","start":{"dateTime":"2026-10-06T10:00:00.0000000"},
            "body":{"contentType":"text","content":""},"bodyPreview":"From the preview"}"#,
    );
    assert!(
        has(&preview, "DESCRIPTION:From the preview"),
        "{preview:#?}"
    );
}

#[test]
fn the_organizer_and_attendees_carry_role_and_answer() {
    let got = lines(
        r#"{"id":"1","start":{"dateTime":"2026-10-06T10:00:00.0000000"},
            "organizer":{"emailAddress":{"name":"Ada Lovelace","address":"ada@example.test"}},
            "attendees":[
              {"type":"required","status":{"response":"accepted"},"emailAddress":{"name":"Grace, H.","address":"grace@example.test"}},
              {"type":"optional","status":{"response":"tentativelyAccepted"},"emailAddress":{"address":"alan@example.test"}},
              {"type":"resource","status":{"response":"none"},"emailAddress":{"name":"Room 4","address":"room4@example.test"}},
              {"type":"required","status":{"response":"declined"},"emailAddress":{"name":"No Address"}}
            ]}"#,
    );
    for want in [
        "ORGANIZER;CN=Ada Lovelace:mailto:ada@example.test",
        "ATTENDEE;CN=\"Grace, H.\";ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:grace@example.test",
        "ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=TENTATIVE:mailto:alan@example.test",
        "ATTENDEE;CN=Room 4;ROLE=NON-PARTICIPANT;PARTSTAT=NEEDS-ACTION:mailto:room4@example.test",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert_eq!(got.iter().filter(|l| l.starts_with("ATTENDEE")).count(), 3);
}

#[test]
fn a_series_master_has_its_rule_and_an_unsayable_pattern_a_comment() {
    let weekly = lines(
        r#"{"id":"1","iCalUId":"s","type":"seriesMaster",
            "start":{"dateTime":"2026-10-05T09:00:00.0000000","timeZone":"Pacific Standard Time"},
            "recurrence":{"pattern":{"type":"weekly","interval":1,"daysOfWeek":["monday"]},"range":{"type":"numbered","numberOfOccurrences":4}}}"#,
    );
    assert!(
        has(&weekly, "RRULE:FREQ=WEEKLY;INTERVAL=1;BYDAY=MO;COUNT=4"),
        "{weekly:#?}"
    );
    let odd = lines(
        r#"{"id":"1","iCalUId":"s","type":"seriesMaster",
            "start":{"dateTime":"2026-10-05T09:00:00.0000000","timeZone":"UTC"},
            "recurrence":{"pattern":{"type":"hourly"},"range":{"type":"noEnd"}}}"#,
    );
    assert!(!odd.iter().any(|l| l.starts_with("RRULE")), "{odd:#?}");
    assert!(
        odd.iter()
            .any(|l| l.starts_with("COMMENT:recurrence pattern hourly is not mirrored")),
        "{odd:#?}"
    );
}

#[test]
fn an_exception_shares_the_series_uid_and_names_the_occurrence_it_replaces() {
    let timed = lines(
        r#"{"id":"EXC","iCalUId":"series-uid","type":"exception","seriesMasterId":"MASTER",
            "originalStart":"2026-10-12T16:00:00Z","subject":"Moved",
            "start":{"dateTime":"2026-10-12T10:00:00.0000000","timeZone":"UTC"}}"#,
    );
    assert!(
        has(&timed, "UID:series-uid") && has(&timed, "RECURRENCE-ID:20261012T160000Z"),
        "{timed:#?}"
    );
    // Without an iCalUId the series master's id is the UID.
    let bare = lines(
        r#"{"id":"EXC","type":"exception","seriesMasterId":"MASTER","originalStart":"2026-10-12T16:00:00Z",
            "start":{"dateTime":"2026-10-12T10:00:00.0000000","timeZone":"UTC"}}"#,
    );
    assert!(has(&bare, "UID:MASTER"), "{bare:#?}");
    let all_day = lines(
        r#"{"id":"EXC","iCalUId":"u","type":"occurrence","isAllDay":true,"originalStart":"2026-10-12T00:00:00Z",
            "start":{"dateTime":"2026-10-12T00:00:00.0000000","timeZone":"UTC"}}"#,
    );
    assert!(
        has(&all_day, "RECURRENCE-ID;VALUE=DATE:20261012"),
        "{all_day:#?}"
    );
}

#[test]
fn what_cannot_be_written_is_an_error_not_a_broken_file() {
    let cases: [(&str, ConvertError); 3] = [
        (
            r#"{"id":"1","@removed":{"reason":"deleted"}}"#,
            ConvertError::Removed,
        ),
        (r#"{"id":"1","subject":"x"}"#, ConvertError::NoStart),
        (
            r#"{"id":"1","start":{"dateTime":"yesterday","timeZone":"UTC"}}"#,
            ConvertError::BadDateTime,
        ),
    ];
    for (json, want) in cases {
        assert_eq!(to_ics(&event(json)).expect_err(json), want, "{json}");
    }
}

#[test]
fn a_long_line_is_folded_inside_75_octets_and_never_inside_a_character() {
    let subject = "é".repeat(100);
    let text = to_ics(&event(&format!(
        r#"{{"id":"1","subject":"{subject}","start":{{"dateTime":"2026-10-06T10:00:00.0000000"}}}}"#
    )))
    .expect("converted");
    for line in text.split("\r\n") {
        assert!(line.len() <= 75, "{} octets: {line}", line.len());
    }
    let unfolded = text.replace("\r\n ", "");
    assert!(unfolded.contains(&format!("SUMMARY:{subject}")));
}

#[test]
fn the_text_helpers_escape_and_strip() {
    assert_eq!(escape("a\\b;c,d\r\ne\nf"), "a\\\\b\\;c\\,d\\ne\\nf");
    assert_eq!(
        html_text("<div>one</div><div>two&nbsp;&lt;3</div>"),
        "one\ntwo <3"
    );
}
