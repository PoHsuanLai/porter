use super::*;
use crate::datasets::pim::{PimKind, is_complete};

fn person(json: &str) -> Person {
    serde_json::from_str(json).expect("person")
}

fn lines(json: &str) -> Vec<String> {
    let text = to_vcf(&person(json));
    assert!(is_complete(PimKind::Contacts, text.as_bytes()), "{text}");
    assert!(text.split("\r\n").all(|l| l.len() <= 75), "{text}");
    text.replace("\r\n ", "")
        .lines()
        .map(str::to_owned)
        .filter(|l| !matches!(l.as_str(), "BEGIN:VCARD" | "END:VCARD"))
        .collect()
}

fn has(lines: &[String], want: &str) -> bool {
    lines.iter().any(|l| l == want)
}

const ADA: &str = r#"{
  "resourceName": "people/c1001", "etag": "%EgQBAj0=",
  "metadata": {"sources": [{"type": "CONTACT"}]},
  "names": [{"displayName": "Ada Lovelace", "familyName": "Lovelace", "givenName": "Ada",
             "middleName": "Augusta", "honorificPrefix": "Countess", "honorificSuffix": "FRS"}],
  "emailAddresses": [{"value": "ada@example.test", "type": "home"},
                     {"value": "ada@engine.test", "type": "work"},
                     {"value": "other@example.test"}, {"value": " "}],
  "phoneNumbers": [{"value": "+44 20 7946 0000", "type": "mobile"},
                   {"value": "+44 20 7946 0001", "type": "workFax"},
                   {"value": "100", "type": "custom"}],
  "addresses": [{"type": "home", "streetAddress": "10 St James's Sq", "city": "London",
                 "region": "", "postalCode": "SW1Y 4JU", "country": "UK"},
                {"type": "work", "formattedValue": "Engine Rd 1\nManchester"}],
  "organizations": [{"name": "Analytical Engines, Ltd", "department": "Research", "title": "Programmer"}],
  "birthdays": [{"date": {"year": 1815, "month": 12, "day": 10}}],
  "biographies": [{"value": "First programmer;\nnotes"}],
  "urls": [{"value": "https://ada.example.test/", "type": "homePage"}]
}"#;

#[test]
fn each_field_of_a_contact_is_written() {
    let got = lines(ADA);
    for want in [
        "UID:people/c1001",
        "FN:Ada Lovelace",
        "N:Lovelace;Ada;Augusta;Countess;FRS",
        "EMAIL;TYPE=INTERNET,HOME:ada@example.test",
        "EMAIL;TYPE=INTERNET,WORK:ada@engine.test",
        "EMAIL;TYPE=INTERNET:other@example.test",
        "TEL;TYPE=CELL:+44 20 7946 0000",
        "TEL;TYPE=WORK,FAX:+44 20 7946 0001",
        "TEL:100",
        "ADR;TYPE=HOME:;;10 St James's Sq;London;;SW1Y 4JU;UK",
        "ADR;TYPE=WORK:;;Engine Rd 1\\, Manchester;;;;",
        "ORG:Analytical Engines\\, Ltd;Research",
        "TITLE:Programmer",
        "BDAY:1815-12-10",
        "NOTE:First programmer\\;\\nnotes",
        "URL:https://ada.example.test/",
        "VERSION:3.0",
    ] {
        assert!(has(&got, want), "{want} in {got:#?}");
    }
    assert_eq!(got.iter().filter(|l| l.starts_with("EMAIL")).count(), 3);
}

#[test]
fn a_card_always_has_a_name_made_from_what_there_is() {
    const CASES: &[(&str, &str)] = &[
        (
            r#"{"resourceName": "people/c1", "names": [{"givenName": "Ada", "familyName": "L"}]}"#,
            "FN:Ada L",
        ),
        (
            r#"{"resourceName": "people/c2", "emailAddresses": [{"value": "x@y.test"}]}"#,
            "FN:x@y.test",
        ),
        (
            r#"{"resourceName": "people/c3", "phoneNumbers": [{"value": "123"}]}"#,
            "FN:123",
        ),
        (
            r#"{"resourceName": "people/c4", "organizations": [{"name": "Acme"}]}"#,
            "FN:Acme",
        ),
        (r#"{"resourceName": "people/c5"}"#, "FN:people/c5"),
        (
            r#"{"resourceName": "people/c6", "names": [{"displayName": "  "}], "organizations": [{"name": "Acme"}]}"#,
            "FN:Acme",
        ),
    ];
    for (json, want) in CASES {
        let got = lines(json);
        assert!(has(&got, want), "{want} in {got:#?}");
        assert!(
            got.iter().any(|l| l.starts_with("N:")),
            "N is required in 3.0"
        );
    }
}

#[test]
fn a_birthday_without_a_year_or_with_a_bad_date_is_handled() {
    const CASES: &[(&str, Option<&str>)] = &[
        (
            r#"{"date": {"month": 12, "day": 10}}"#,
            Some("BDAY:--12-10"),
        ),
        (
            r#"{"date": {"year": 1815, "month": 12, "day": 10}}"#,
            Some("BDAY:1815-12-10"),
        ),
        (
            r#"{"date": {"year": 0, "month": 2, "day": 29}}"#,
            Some("BDAY:--02-29"),
        ),
        (r#"{"date": {"year": 1990, "month": 13, "day": 1}}"#, None),
        (r#"{"date": {"year": 1990, "month": 5}}"#, None),
        (r#"{}"#, None),
    ];
    for (birthday, want) in CASES {
        let got = lines(&format!(
            r#"{{"resourceName": "people/c1", "birthdays": [{birthday}]}}"#
        ));
        match want {
            Some(want) => assert!(has(&got, want), "{want} in {got:#?}"),
            None => assert!(!got.iter().any(|l| l.starts_with("BDAY")), "{got:#?}"),
        }
    }
}

#[test]
fn a_deleted_contact_is_known_by_its_metadata() {
    assert!(person(r#"{"resourceName": "people/c1", "metadata": {"deleted": true}}"#).is_deleted());
    assert!(
        !person(r#"{"resourceName": "people/c1", "metadata": {"deleted": false}}"#).is_deleted()
    );
    assert!(!person(ADA).is_deleted());
}

#[test]
fn a_long_note_is_folded_and_unfolds_to_the_same_text() {
    let note = "é".repeat(120);
    let json =
        format!(r#"{{"resourceName": "people/c1", "biographies": [{{"value": "{note}"}}]}}"#);
    let raw = to_vcf(&person(&json));
    assert!(raw.contains("\r\n "), "folded");
    assert!(has(&lines(&json), &format!("NOTE:{note}")));
}
