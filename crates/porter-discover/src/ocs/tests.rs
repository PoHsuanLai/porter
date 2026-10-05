//! Nextcloud's capabilities answer, from recorded-shape fixtures.

use super::*;
use porter_core::capability::Capability;

fn offers(found: &Found) -> Vec<(CapabilityKind, bool)> {
    found
        .claims
        .iter()
        .map(|c| (c.offer.kind(), matches!(c.offer, Offer::Present(_))))
        .collect()
}

#[test]
fn a_full_install_offers_every_kind() {
    let found = parse_ocs_capabilities(include_str!("../../fixtures/ocs-capabilities.json"))
        .expect("found");
    assert_eq!(found.source, Source::Ocs);
    assert!(found.endpoints.is_empty());
    assert!(
        found
            .claims
            .iter()
            .all(|c| c.provenance == Provenance::Discovered)
    );
    assert_eq!(
        offers(&found),
        [
            (CapabilityKind::Storage, true),
            (CapabilityKind::Calendar, true),
            (CapabilityKind::Contacts, true),
            (CapabilityKind::Tasks, false),
            (CapabilityKind::Notes, true),
        ]
    );
}

#[test]
fn chunked_upload_follows_the_dav_chunking_advertisement() {
    let chunked = |text: &str| {
        let found = parse_ocs_capabilities(text).expect("found");
        let Offer::Present(Capability::Storage(storage)) = &found.claims[0].offer else {
            panic!("{:?}", found.claims[0])
        };
        storage.chunked_upload
    };
    assert_eq!(
        chunked(include_str!("../../fixtures/ocs-capabilities.json")),
        Offered::Present
    );
    assert_eq!(
        chunked(include_str!("../../fixtures/ocs-minimal.json")),
        Offered::Absent
    );
}

#[test]
fn an_install_without_the_apps_marks_them_not_on_server() {
    let found =
        parse_ocs_capabilities(include_str!("../../fixtures/ocs-minimal.json")).expect("found");
    assert_eq!(
        offers(&found),
        [
            (CapabilityKind::Storage, true),
            (CapabilityKind::Calendar, false),
            (CapabilityKind::Contacts, false),
            (CapabilityKind::Tasks, false),
            (CapabilityKind::Notes, false),
        ]
    );
    assert!(found.claims[1..].iter().all(|c| matches!(
        c.offer,
        Offer::Absent {
            reason: AbsentReason::NotOnServer,
            ..
        }
    )));
}

#[test]
fn the_calendar_app_brings_tasks_and_the_dav_app_brings_calendars_and_contacts() {
    let json =
        r#"{"ocs":{"meta":{"statuscode":100},"data":{"capabilities":{"calendar":{},"files":{}}}}}"#;
    let found = parse_ocs_capabilities(json).expect("found");
    let present: Vec<CapabilityKind> = found
        .claims
        .iter()
        .filter(|c| matches!(c.offer, Offer::Present(_)))
        .map(|c| c.offer.kind())
        .collect();
    assert_eq!(
        present,
        [
            CapabilityKind::Storage,
            CapabilityKind::Calendar,
            CapabilityKind::Tasks
        ]
    );
}

#[test]
fn an_error_status_is_no_servers_and_anything_else_unreadable() {
    let denied = r#"{"ocs":{"meta":{"status":"failure","statuscode":997},"data":[]}}"#;
    assert_eq!(
        parse_ocs_capabilities(denied),
        Err(DiscoverFault::NoServers)
    );
    for text in [
        "",
        "<html/>",
        "{}",
        r#"{"ocs":{}}"#,
        r#"{"ocs":{"meta":{"statuscode":200},"data":{}}}"#,
        r#"{"ocs":{"meta":{"statuscode":200},"data":{"capabilities":[]}}}"#,
    ] {
        assert_eq!(
            parse_ocs_capabilities(text),
            Err(DiscoverFault::Unreadable),
            "{text:?}"
        );
    }
}
