use super::*;
use crate::datasets::pim::{PimKind, is_complete};

fn task(json: &str) -> Task {
    serde_json::from_str(json).expect("task")
}

fn lines(json: &str) -> Vec<String> {
    let text = to_ics(&task(json));
    assert!(is_complete(PimKind::Tasks, text.as_bytes()), "{text}");
    assert!(text.ends_with("\r\n"));
    text.replace("\r\n ", "")
        .lines()
        .map(str::to_owned)
        .filter(|l| {
            !matches!(
                l.as_str(),
                "BEGIN:VCALENDAR" | "END:VCALENDAR" | "BEGIN:VTODO" | "END:VTODO"
            )
        })
        .collect()
}

fn has(lines: &[String], want: &str) -> bool {
    lines.iter().any(|l| l == want)
}

#[test]
fn an_open_task_is_needs_action_with_its_due_date() {
    let got = lines(
        r#"{"kind": "tasks#task", "id": "t1", "etag": "\"x\"", "title": "Buy milk, eggs",
            "notes": "Semi;skimmed\nand a bag", "status": "needsAction",
            "due": "2026-10-12T00:00:00.000Z", "updated": "2026-10-08T07:21:03.000Z"}"#,
    );
    for want in [
        "UID:t1",
        "DTSTAMP:20261008T072103Z",
        "LAST-MODIFIED:20261008T072103Z",
        "SUMMARY:Buy milk\\, eggs",
        "DESCRIPTION:Semi\\;skimmed\\nand a bag",
        "DUE;VALUE=DATE:20261012",
        "STATUS:NEEDS-ACTION",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert!(!got.iter().any(|l| l.starts_with("COMPLETED")));
}

#[test]
fn a_completed_task_says_when_and_a_subtask_names_its_parent() {
    let got = lines(
        r#"{"id": "t2", "title": "Sub", "status": "completed",
            "completed": "2026-10-10T09:15:00.000Z", "parent": "t1",
            "updated": "2026-10-10T09:15:00.000Z"}"#,
    );
    for want in [
        "STATUS:COMPLETED",
        "COMPLETED:20261010T091500Z",
        "RELATED-TO;RELTYPE=PARENT:t1",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
}

#[test]
fn a_task_with_little_still_makes_a_whole_item() {
    let got = lines(r#"{"id": "t3"}"#);
    assert!(has(&got, "UID:t3") && has(&got, "STATUS:NEEDS-ACTION"));
    assert!(has(&got, "DTSTAMP:19700101T000000Z"));
    assert!(
        !got.iter()
            .any(|l| l.starts_with("SUMMARY") || l.starts_with("DUE"))
    );
    // `completed` on a task that is open again is not carried.
    let reopened =
        lines(r#"{"id": "t4", "status": "needsAction", "completed": "2026-10-10T09:15:00.000Z"}"#);
    assert!(
        !reopened.iter().any(|l| l.starts_with("COMPLETED")),
        "{reopened:#?}"
    );
    // A due date that is not a date is left out.
    assert!(
        !lines(r#"{"id": "t5", "due": "soon"}"#)
            .iter()
            .any(|l| l.starts_with("DUE"))
    );
}

#[test]
fn a_deleted_task_is_known_by_its_flag() {
    assert!(task(r#"{"id": "t", "deleted": true}"#).is_deleted());
    assert!(!task(r#"{"id": "t", "deleted": false}"#).is_deleted());
    assert!(!task(r#"{"id": "t"}"#).is_deleted());
}
