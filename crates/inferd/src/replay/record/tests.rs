use super::*;
use crate::testkit::Scratch;
use serde_json::json;
use std::os::unix::fs::MetadataExt;

#[test]
fn bodies_are_appended_one_json_line_each_in_a_0600_file() {
    let dir = Scratch::new("record");
    let path = dir.path().join("sent.jsonl");
    let recorder = Recorder::new(path.clone());
    recorder.append(&json!({"messages": [{"role": "user", "content": "hi\nthere"}]}));
    recorder.append(&json!({"stream": true}));
    let text = std::fs::read_to_string(&path).expect("file");
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1], json!({"stream": true}));
    assert_eq!(lines[0]["messages"][0]["content"], "hi\nthere");
    assert_eq!(
        std::fs::metadata(&path).expect("meta").mode() & 0o777,
        0o600
    );
}

#[test]
fn a_wider_file_that_was_there_is_tightened() {
    let dir = Scratch::new("record-wide");
    let path = dir.path().join("sent.jsonl");
    std::fs::write(&path, "").expect("file");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
    Recorder::new(path.clone()).append(&json!({}));
    assert_eq!(
        std::fs::metadata(&path).expect("meta").mode() & 0o777,
        0o600
    );
}
