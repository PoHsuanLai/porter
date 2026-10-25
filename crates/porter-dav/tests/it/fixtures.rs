//! Recorded multistatus bodies (Nextcloud, Fastmail, a generic server) through the discovery
//! and quota readers, with no network. The bodies are dav-proto's fixtures, which also test the
//! parsing and the sync-collection reading.

use dav_proto::{Multistatus, parse_multistatus};
use porter_dav::{Home, Quota, collections, current_user_principal, home_set, quota};

fn fixture(name: &str) -> Multistatus {
    let path = format!(
        "{}/../dav-proto/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    parse_multistatus(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
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
fn fastmail_discovery() {
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
fn a_collection_is_found_beside_failed_members() {
    let ms = fixture("generic-mixed.xml");
    assert_eq!(collections(&ms, Home::Calendar), ["/dav/cal/work/"]);
}
