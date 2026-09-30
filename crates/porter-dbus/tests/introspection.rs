//! The checked-in introspection files are the interfaces the skeletons declare. A change to a
//! signature changes the file in the same commit.

use porter_dbus::{Bus, introspection};
use std::path::PathBuf;

#[test]
fn checked_in_introspection_matches_the_interfaces() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dbus");
    for bus in [Bus::Accounts, Bus::Sync, Bus::Inference] {
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
