//! The checked-in introspection files are the interfaces the skeletons declare. A change to a
//! signature changes the file in the same commit.

use porter_dbus::{Bus, introspection};
use std::path::PathBuf;

#[test]
fn checked_in_introspection_matches_the_interfaces() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dbus");
    for bus in [Bus::Accounts, Bus::AccountsSheet, Bus::Sync, Bus::Inference] {
        let path = dir.join(bus.file_name());
        let expected =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = introspection(bus);
        assert!(
            actual == expected,
            "{} differs from the interfaces; it should read:\n{actual}",
            path.display()
        );
    }
}

#[test]
fn every_accounts_member_is_declared() {
    let xml = introspection(Bus::Accounts);
    let members = [
        "<method name=\"Query\">",
        "<method name=\"Availability\">",
        "<method name=\"Choose\">",
        "<method name=\"AddAccount\">",
        "<method name=\"Reauthenticate\">",
        "<method name=\"List\">",
        "<method name=\"Revoke\">",
        "<method name=\"IssueToken\">",
        "<method name=\"OpenAuthenticated\">",
        "<method name=\"OpenLinked\">",
        "<method name=\"Close\">",
        "<signal name=\"AccountAdded\">",
        "<signal name=\"AccountRemoved\">",
        "<signal name=\"CapabilityChanged\">",
        "<signal name=\"NeedsReauth\">",
        "<signal name=\"GrantChanged\">",
        "<signal name=\"Response\">",
        "<property name=\"Capabilities\" type=\"a(sa{sv})\" access=\"read\"/>",
    ];
    for member in members {
        assert!(xml.contains(member), "missing {member}");
    }
}

#[test]
fn inference1_xml_unchanged_members() {
    let xml = introspection(Bus::Inference);
    let members = [
        "<method name=\"Availability\">",
        "<method name=\"Open\">",
        "<method name=\"Prepare\">",
        "<method name=\"Usage\">",
        "<method name=\"Rescan\">",
        "<signal name=\"EnginesChanged\">",
        "<property name=\"Gpu\" type=\"s\" access=\"read\"/>",
    ];
    for member in members {
        assert!(xml.contains(member), "missing {member}");
    }
    let declared = xml.matches("<method ").count()
        + xml.matches("<signal ").count()
        + xml.matches("<property ").count();
    assert_eq!(declared, members.len(), "no member beyond the frozen seven");
}

#[test]
fn speech_and_computer_use_are_fd_only() {
    let xml = introspection(Bus::Inference);
    for word in ["Transcribe", "Speak", "Audio", "Cua", "Listen"] {
        assert!(
            !xml.contains(word),
            "{word} must travel on the Open fd, not as a member"
        );
    }
}

#[test]
fn inference_calls_carry_an_options_dict_for_the_reserved_traceparent() {
    let xml = introspection(Bus::Inference);
    assert_eq!(
        xml.matches("<arg name=\"options\" type=\"a{sv}\" direction=\"in\"/>")
            .count(),
        3,
        "Availability, Open and Prepare"
    );
    assert_eq!(porter_dbus::OPTION_TRACEPARENT, "traceparent");
}

#[test]
fn the_peer_interface_is_the_three_daemon_to_daemon_members() {
    let xml = introspection(Bus::Accounts);
    let peer = xml
        .split("<interface name=\"org.quire.Accounts1.Peer\">")
        .nth(1)
        .and_then(|rest| rest.split("</interface>").next())
        .expect("the Peer interface is declared");
    for member in [
        "<method name=\"Verdicts\">",
        "<method name=\"ResolveKey\">",
        "<method name=\"ReportLocal\">",
        "<method name=\"SetAgentState\">",
    ] {
        assert!(peer.contains(member), "missing {member}");
    }
    assert_eq!(peer.matches("<method ").count(), 4);
    assert!(
        !peer.contains("OpenCredential"),
        "syncd opens an authenticated stream like any app"
    );
    assert!(
        peer.contains("<arg type=\"h\" direction=\"out\"/>"),
        "a key travels on a descriptor, never a string"
    );
}

#[test]
fn the_sheet_backend_takes_views_in_and_sends_inputs_out() {
    let xml = introspection(Bus::AccountsSheet);
    for member in [
        "<method name=\"Open\">",
        "<method name=\"Update\">",
        "<method name=\"Close\">",
        "<signal name=\"Input\">",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    let declared = xml.matches("<method ").count() + xml.matches("<signal ").count();
    assert_eq!(declared, 4);
    assert_eq!(porter_dbus::SHEET_BUS, "org.quire.AccountsSheet1");
}

#[test]
fn syncd_and_inferd_declare_what_they_did() {
    let sync = introspection(Bus::Sync);
    assert_eq!(sync.matches("<method ").count(), 5);
    assert!(sync.contains("<method name=\"Resolve\">"));
    assert!(sync.contains("<arg name=\"conflict\" type=\"x\" direction=\"in\"/>"));
    assert_eq!(porter_dbus::RESOLVE_KEEP_LOCAL, "keep_local");
    assert_eq!(porter_dbus::RESOLVE_KEEP_REMOTE, "keep_remote");
    assert_eq!(
        porter_dbus::SYNC_ERROR_NO_SUCH_CONFLICT,
        format!("{}NoSuchConflict", porter_dbus::SYNC_ERROR_PREFIX)
    );
    assert_eq!(porter_dbus::STATUS_KEY_QUOTA, "quota");
}
