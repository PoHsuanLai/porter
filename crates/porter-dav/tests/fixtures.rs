//! Recorded multistatus bodies (Nextcloud, Fastmail, a generic server), read with no network.

use porter_dav::{
    Home, PropStatus, Quota, SyncChange, collections, current_user_principal, home_set,
    parse_multistatus, parse_sync_collection, quota, token_expired,
};

fn fixture(name: &str) -> porter_dav::Multistatus {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    parse_multistatus(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn changed(href: &str, etag: &str) -> SyncChange {
    SyncChange::Changed {
        href: href.into(),
        etag: etag.into(),
    }
}

fn removed(href: &str) -> SyncChange {
    SyncChange::Removed { href: href.into() }
}

#[test]
fn nextcloud_discovery() {
    let principal = fixture("nextcloud-principal.xml");
    assert_eq!(
        current_user_principal(&principal).as_deref(),
        Some("/remote.php/dav/principals/users/alice/")
    );
    let homes = fixture("nextcloud-homes.xml");
    assert_eq!(
        home_set(&homes, Home::Calendar),
        ["/remote.php/dav/calendars/alice/"]
    );
    assert_eq!(
        home_set(&homes, Home::Addressbook),
        ["/remote.php/dav/addressbooks/users/alice/"]
    );
    let calendars = fixture("nextcloud-calendars.xml");
    assert_eq!(
        collections(&calendars, Home::Calendar),
        ["/remote.php/dav/calendars/alice/personal/"]
    );
    assert!(collections(&calendars, Home::Addressbook).is_empty());
}

#[test]
fn nextcloud_sync_with_changes_and_a_removal() {
    let reply = parse_sync_collection(&fixture("nextcloud-sync.xml")).unwrap();
    assert_eq!(
        reply.changes,
        [
            changed(
                "/remote.php/dav/calendars/alice/personal/new-event.ics",
                "\"4e1a7b0c2f9d5a3e8c6b1d0f7a2e9c44\""
            ),
            changed(
                "/remote.php/dav/calendars/alice/personal/moved%20meeting.ics",
                "\"b7c3d2e1a0f94e5d8c7b6a5f4e3d2c1b\""
            ),
            removed("/remote.php/dav/calendars/alice/personal/cancelled.ics"),
        ]
    );
    assert_eq!(
        reply.sync_token.as_deref(),
        Some("http://nextcloud.example.test/ns/sync/4821")
    );
}

#[test]
fn nextcloud_quota_present() {
    assert_eq!(
        quota(&fixture("nextcloud-quota.xml")),
        Quota {
            used: Some(5_368_709),
            available: Some(10_737_418_240)
        }
    );
}

#[test]
fn fastmail_discovery_and_sync() {
    let principal = fixture("fastmail-principal.xml");
    assert_eq!(
        current_user_principal(&principal).as_deref(),
        Some("/dav/principals/user/alice@fastmail.example/")
    );
    assert_eq!(
        home_set(&principal, Home::Calendar),
        ["/dav/calendars/user/alice@fastmail.example/"]
    );
    assert_eq!(
        home_set(&principal, Home::Addressbook),
        ["/dav/addressbooks/user/alice@fastmail.example/"]
    );
    let reply = parse_sync_collection(&fixture("fastmail-sync.xml")).unwrap();
    let base = "/dav/addressbooks/user/alice@fastmail.example/Default";
    assert_eq!(
        reply.changes,
        [
            changed(&format!("{base}/6f1c.vcf"), "\"1b2c3d\""),
            changed(&format!("{base}/91aa.vcf"), "\"4e5f6a\""),
            removed(&format!("{base}/dead.vcf")),
            removed(&format!("{base}/dead2.vcf")),
        ]
    );
    assert_eq!(reply.sync_token.as_deref(), Some("data:,2f9a1c"));
}

#[test]
fn fastmail_quota_with_only_the_used_half() {
    assert_eq!(
        quota(&fixture("fastmail-quota.xml")),
        Quota {
            used: Some(48_213),
            available: None
        }
    );
}

#[test]
fn quota_absent_altogether() {
    assert_eq!(quota(&fixture("generic-sync-first.xml")), Quota::default());
}

#[test]
fn generic_first_listing_has_a_token_and_no_removals() {
    let reply = parse_sync_collection(&fixture("generic-sync-first.xml")).unwrap();
    assert_eq!(
        reply.changes,
        [
            changed("/dav/cal/work/a.ics", "\"a1\""),
            changed("/dav/cal/work/b.ics", "\"b1\"")
        ]
    );
    assert_eq!(reply.sync_token.as_deref(), Some("urn:sync:1"));
}

#[test]
fn an_expired_token_is_an_error_body_not_a_multistatus() {
    let body = std::fs::read_to_string(format!(
        "{}/tests/fixtures/generic-sync-expired.xml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(parse_multistatus(&body).is_err());
    assert!(token_expired(403, &body));
    assert!(token_expired(409, &body));
    assert!(!token_expired(500, &body));
}

#[test]
fn a_507_member_makes_the_anchor_expired() {
    let reply = parse_sync_collection(&fixture("generic-sync-truncated.xml")).unwrap();
    assert_eq!(reply.changes, [changed("/dav/cal/work/a.ics", "\"a2\"")]);
    assert_eq!(reply.sync_token, None);
}

#[test]
fn a_207_with_mixed_statuses_per_href_and_per_property() {
    let ms = fixture("generic-mixed.xml");
    let rs = &ms.responses;
    assert_eq!(rs.len(), 3);
    let work = &rs[0];
    assert_eq!(work.found("DAV:displayname"), Some("Work"));
    assert_eq!(
        work.prop("DAV:getetag").unwrap().status,
        PropStatus::Missing
    );
    assert_eq!(
        work.prop("DAV:sync-token").unwrap().status,
        PropStatus::Other(403)
    );
    assert_eq!(work.status, None);
    assert_eq!(rs[1].status, Some(403));
    assert_eq!(rs[2].status, Some(404));
    assert_eq!(collections(&ms, Home::Calendar), ["/dav/cal/work/"]);
    // Sync reads the same body: the 404 is a removal, the 403 is not a change, no token.
    let reply = parse_sync_collection(&ms).unwrap();
    assert_eq!(
        reply.changes,
        [changed("/dav/cal/work/", ""), removed("/dav/cal/gone/")]
    );
    assert_eq!(reply.sync_token, None);
}
