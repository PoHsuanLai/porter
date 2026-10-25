//! Recorded multistatus bodies (Nextcloud, Fastmail, a generic server), read with no network.

use dav_proto::carddav::Props;
use dav_proto::names::{
    CALENDAR_HOME_SET, CURRENT_USER_PRINCIPAL, DISPLAYNAME, GETETAG, QUOTA_AVAILABLE, QUOTA_USED,
    RESOURCETYPE, SYNC_TOKEN,
};
use dav_proto::{
    Error, Multistatus, PropStatus, SyncChange, SyncEnd, parse_multistatus, parse_sync_collection,
    token_expired,
};

fn text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn fixture(name: &str) -> Multistatus {
    parse_multistatus(&text(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
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
fn nextcloud_principal_and_home_sets() {
    let principal = fixture("nextcloud-principal.xml");
    assert_eq!(
        principal.responses[0].found(CURRENT_USER_PRINCIPAL),
        Some("/remote.php/dav/principals/users/alice/")
    );
    let homes = fixture("nextcloud-homes.xml");
    let home = &homes.responses[0];
    assert_eq!(
        home.found(CALENDAR_HOME_SET),
        Some("/remote.php/dav/calendars/alice/")
    );
    assert_eq!(
        Props::of(home).addressbook_home_set,
        ["/remote.php/dav/addressbooks/users/alice/"]
    );
}

#[test]
fn nextcloud_calendars_name_their_kinds() {
    let calendars = fixture("nextcloud-calendars.xml");
    let kinds: Vec<(&str, Option<&str>)> = calendars
        .responses
        .iter()
        .map(|r| (r.href.as_str(), r.found(RESOURCETYPE)))
        .collect();
    assert_eq!(
        kinds,
        [
            ("/remote.php/dav/calendars/alice/", Some("DAV:collection")),
            (
                "/remote.php/dav/calendars/alice/personal/",
                Some("DAV:collection urn:ietf:params:xml:ns:caldav:calendar")
            ),
            (
                "/remote.php/dav/calendars/alice/inbox/",
                Some("DAV:collection urn:ietf:params:xml:ns:caldav:schedule-inbox")
            ),
        ]
    );
    assert_eq!(calendars.responses[1].found(DISPLAYNAME), Some("Personal"));
}

#[test]
fn nextcloud_sync_with_changes_and_a_removal() {
    let reply = parse_sync_collection(&fixture("nextcloud-sync.xml"));
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
        reply.end,
        SyncEnd::Complete("http://nextcloud.example.test/ns/sync/4821".into())
    );
}

#[test]
fn nextcloud_quota_present() {
    let quota = fixture("nextcloud-quota.xml");
    assert_eq!(quota.responses[0].found(QUOTA_USED), Some("5368709"));
    assert_eq!(
        quota.responses[0].found(QUOTA_AVAILABLE),
        Some("10737418240")
    );
}

#[test]
fn fastmail_discovery_in_one_answer() {
    let principal = fixture("fastmail-principal.xml");
    let props = Props::of(&principal.responses[0]);
    assert_eq!(
        props.current_user_principal.as_deref(),
        Some("/dav/principals/user/alice@fastmail.example/")
    );
    assert_eq!(
        props.addressbook_home_set,
        ["/dav/addressbooks/user/alice@fastmail.example/"]
    );
    assert_eq!(
        principal.responses[0].found(CALENDAR_HOME_SET),
        Some("/dav/calendars/user/alice@fastmail.example/")
    );
}

#[test]
fn fastmail_sync_with_two_removals() {
    let reply = parse_sync_collection(&fixture("fastmail-sync.xml"));
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
    assert_eq!(reply.end, SyncEnd::Complete("data:,2f9a1c".into()));
}

#[test]
fn fastmail_quota_with_only_the_used_half() {
    let quota = fixture("fastmail-quota.xml");
    let r = &quota.responses[0];
    assert_eq!(r.found(QUOTA_USED), Some("48213"));
    assert_eq!(r.found(QUOTA_AVAILABLE), None);
    assert_eq!(r.prop(QUOTA_AVAILABLE).unwrap().status, PropStatus::Missing);
}

#[test]
fn generic_first_listing_has_a_token_and_no_removals() {
    let status = fixture("generic-sync-first.xml");
    assert_eq!(status.responses[0].found(QUOTA_USED), None);
    let reply = parse_sync_collection(&status);
    assert_eq!(
        reply.changes,
        [
            changed("/dav/cal/work/a.ics", "\"a1\""),
            changed("/dav/cal/work/b.ics", "\"b1\"")
        ]
    );
    assert_eq!(reply.end, SyncEnd::Complete("urn:sync:1".into()));
}

#[test]
fn an_expired_token_is_an_error_body_not_a_multistatus() {
    let body = text("generic-sync-expired.xml");
    assert_eq!(parse_multistatus(&body), Err(Error::NotMultistatus));
    assert!(token_expired(403, &body));
    assert!(token_expired(409, &body));
    assert!(token_expired(412, &body));
    assert!(!token_expired(500, &body));
    assert!(!token_expired(507, &body));
}

#[test]
fn a_507_member_is_a_truncation_to_continue_from_not_an_expired_token() {
    let reply = parse_sync_collection(&fixture("generic-sync-truncated.xml"));
    assert_eq!(reply.changes, [changed("/dav/cal/work/a.ics", "\"a2\"")]);
    assert_eq!(reply.end, SyncEnd::Truncated("urn:sync:partial".into()));
    assert_eq!(reply.token(), Some("urn:sync:partial"));
}

#[test]
fn a_207_with_mixed_statuses_per_href_and_per_property() {
    let ms = fixture("generic-mixed.xml");
    let rs = &ms.responses;
    assert_eq!(rs.len(), 3);
    let work = &rs[0];
    assert_eq!(work.found(DISPLAYNAME), Some("Work"));
    assert_eq!(work.prop(GETETAG).unwrap().status, PropStatus::Missing);
    assert_eq!(work.prop(QUOTA_USED).unwrap().status, PropStatus::Missing);
    assert_eq!(
        work.prop(SYNC_TOKEN).unwrap().status,
        PropStatus::Other(403)
    );
    assert_eq!(work.status, None);
    assert_eq!(rs[1].status, Some(403));
    assert_eq!(rs[2].status, Some(404));
    assert!(rs[2].gone());
    // The typed view leaves out what failed.
    let props = Props::of(work);
    assert_eq!(props.etag, None);
    assert_eq!(props.sync_token, None);
    assert_eq!(props.display_name.as_deref(), Some("Work"));
    // Sync reads the same body: the 404 is a removal, the 403 is not a change, no token.
    let reply = parse_sync_collection(&ms);
    assert_eq!(
        reply.changes,
        [changed("/dav/cal/work/", ""), removed("/dav/cal/gone/")]
    );
    assert_eq!(reply.end, SyncEnd::Missing);
}
