//! The checked-in introspection files are the interfaces as they are declared once. A change to a
//! signature changes the file in the same commit.

use porter_dbus::accounts;
use std::path::PathBuf;

#[test]
fn checked_in_introspection_matches_the_interfaces() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dbus");
    for document in [
        accounts::ACCOUNTS,
        accounts::ACCOUNTS_SHEET,
        accounts::KEYS,
        accounts::GRANTS,
        accounts::LAUNCHER,
        accounts::BROKER,
        accounts::SPACES,
        accounts::IDENTITIES,
        accounts::TAILNET,
    ] {
        let path = dir.join(document.file_name);
        let expected =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = document.introspection();
        assert!(
            actual == expected,
            "{} differs from the interfaces; it should read:\n{actual}",
            path.display()
        );
    }
}

#[test]
fn the_spaces_registry_declares_its_six_methods_and_one_signal() {
    let xml = porter_dbus::accounts::SPACES.introspection();
    for member in [
        "<method name=\"List\">",
        "<arg type=\"a(sa{sv})\" direction=\"out\"/>",
        "<method name=\"Create\">",
        "<method name=\"Register\">",
        "<method name=\"Rename\">",
        "<method name=\"SetLook\">",
        "<method name=\"Remove\">",
        "<signal name=\"Changed\">",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    assert_eq!(xml.matches("<method ").count(), 6);
    assert_eq!(xml.matches("<signal ").count(), 1);
    assert_eq!(porter_wire::names::SPACES_PATH, "/org/quire/Spaces1");
    assert!(
        !porter_dbus::accounts::ACCOUNTS
            .introspection()
            .contains("org.quire.Spaces1"),
        "its own file, beside Accounts1's"
    );
}

#[test]
fn the_identities_object_declares_five_methods_and_one_signal_and_no_token() {
    let xml = porter_dbus::accounts::IDENTITIES.introspection();
    for member in [
        "<interface name=\"org.quire.Identities1\">",
        "<method name=\"List\">",
        "<arg type=\"a(sa{sv})\" direction=\"out\"/>",
        "<method name=\"Offer\">",
        "<arg name=\"provider_hint\" type=\"s\" direction=\"in\"/>",
        "<method name=\"TakePassword\">",
        // The secret travels on a descriptor, never in a string.
        "<arg type=\"h\" direction=\"out\"/>",
        "<method name=\"RegisterUse\">",
        "<method name=\"Uses\">",
        "<arg type=\"as\" direction=\"out\"/>",
        "<signal name=\"IdentityRemoved\">",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    assert_eq!(xml.matches("<method ").count(), 5);
    assert_eq!(xml.matches("<signal ").count(), 1);
    assert_eq!(xml.matches("<property ").count(), 0);
    // `Offer` answers its identity's id, address and provider, and nothing that is a token.
    assert_eq!(xml.matches("direction=\"out\"").count(), 6);
    assert_eq!(
        porter_wire::names::IDENTITIES_PATH,
        "/org/quire/Identities1"
    );
    // Additive: its own file, and the others are as they were.
    assert!(
        !porter_dbus::accounts::ACCOUNTS
            .introspection()
            .contains("Identities1")
    );
    assert!(
        !porter_dbus::accounts::SPACES
            .introspection()
            .contains("Identities1")
    );
    assert!(
        !porter_dbus::accounts::TAILNET
            .introspection()
            .contains("Identities1")
    );
}

#[test]
fn the_tailnet_object_declares_machines_and_changed_and_nothing_else() {
    let xml = porter_dbus::accounts::TAILNET.introspection();
    for member in [
        "<interface name=\"org.quire.Tailnet1\">",
        "<method name=\"Machines\">",
        "<arg type=\"a(sa{sv})\" direction=\"out\"/>",
        "<signal name=\"Changed\">",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    assert_eq!(xml.matches("<method ").count(), 1);
    assert_eq!(xml.matches("<signal ").count(), 1);
    assert_eq!(xml.matches("<property ").count(), 0);
    // The signal carries nothing: a listener calls Machines again.
    assert!(xml.contains("<signal name=\"Changed\">\n   </signal>"));
    assert_eq!(porter_wire::names::TAILNET_PATH, "/org/quire/Tailnet1");
    // Additive: its own file, and Accounts1's and Spaces1's are as they were.
    assert!(
        !porter_dbus::accounts::ACCOUNTS
            .introspection()
            .contains("Tailnet1")
    );
    assert!(
        !porter_dbus::accounts::SPACES
            .introspection()
            .contains("Tailnet1")
    );
}

#[test]
fn accountd_declares_identities_and_no_grant_keyed_member() {
    let xml = porter_dbus::accounts::ACCOUNTS.introspection();
    let members = [
        "<method name=\"AddAccount\">",
        "<method name=\"NeedingReauth\">",
        "<method name=\"Reauthenticate\">",
        "<method name=\"Close\">",
        "<signal name=\"AccountAdded\">",
        "<signal name=\"AccountRemoved\">",
        "<signal name=\"Response\">",
        "<property name=\"Capabilities\" type=\"a(sa{sv})\" access=\"read\"/>",
    ];
    for member in members {
        assert!(xml.contains(member), "missing {member}");
    }
    // Whoever uses a credential holds it: nothing here finds, grants, relays or issues one.
    for gone in [
        "Query",
        "Availability",
        "Choose",
        "Revoke",
        "IssueToken",
        "OpenAuthenticated",
        "OpenLinked",
        "IssueProcessCredential",
        "CapabilityChanged",
        "GrantChanged",
        "Accounts1.Grants",
        "Accounts1.Tokens",
        "Accounts1.Peer",
    ] {
        assert!(!xml.contains(gone), "{gone} is not accountd's any more");
    }
}

#[test]
fn the_sheet_backend_takes_views_in_and_sends_inputs_out() {
    let xml = porter_dbus::accounts::ACCOUNTS_SHEET.introspection();
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
    assert_eq!(porter_wire::names::SHEET_BUS, "org.quire.AccountsSheet1");
}

#[test]
fn accountds_launcher_is_the_old_ones_eight_methods_and_three_signals_with_a_memfd_reply() {
    let xml = porter_dbus::accounts::LAUNCHER.introspection();
    for member in [
        "<interface name=\"org.quire.Accounts1.Launcher\">",
        "<method name=\"RegisterLauncher\">",
        "<method name=\"BeginSession\">",
        "<method name=\"EndSession\">",
        "<method name=\"RequestAgentKey\">",
        "<method name=\"ReportAgentLogin\">",
        "<method name=\"ReportAgentLogout\">",
        "<method name=\"IssueProcessCredential\">",
        "<method name=\"RevokeProcessCredential\">",
        "<signal name=\"AgentLoginRequested\">",
        "<signal name=\"AgentLogoutRequested\">",
        "<signal name=\"ProcessCredentialRevoked\">",
        "<arg name=\"permit\" type=\"s\" direction=\"in\"/>",
        // The key travels on a sealed descriptor, in no string.
        "<arg type=\"h\" direction=\"out\"/>",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    assert_eq!(xml.matches("<method ").count(), 8);
    assert_eq!(xml.matches("<signal ").count(), 3);
    assert!(!xml.contains("SetAgentState"), "its effect is the reports'");
    assert!(!xml.contains("type=\"v\""), "the handle is a descriptor");
    assert!(
        !xml.contains("name=\"kind\""),
        "RequestAgentKey has no kind"
    );
    assert!(!xml.contains("grant"), "the launcher speaks of permits");
}

#[test]
fn accountds_keys_interface_is_a_request_for_a_permit_and_nothing_else() {
    let xml = porter_dbus::accounts::KEYS.introspection();
    assert!(xml.contains("<interface name=\"org.quire.Accounts1.Keys\">"));
    assert!(xml.contains("<method name=\"RequestKey\">"));
    // Adding a key is `Manager.AddAccount`; a Request object comes back.
    assert_eq!(xml.matches("<method ").count(), 1);
    assert_eq!(xml.matches("<signal ").count(), 0);
    assert_eq!(
        xml.matches("<arg type=\"o\" direction=\"out\"/>").count(),
        1
    );
}

#[test]
fn the_broker_declares_six_methods_and_one_signal_and_hands_keys_only_on_descriptors() {
    let xml = porter_dbus::accounts::BROKER.introspection();
    for member in [
        "<interface name=\"org.quire.Accounts1.Broker\">",
        "<method name=\"Verdicts\">",
        "<arg name=\"app\" type=\"(ss)\" direction=\"in\"/>",
        "<method name=\"ReadKey\">",
        "<method name=\"Accounts\">",
        "<method name=\"Servers\">",
        "<method name=\"ReadServerKey\">",
        "<method name=\"ReportLocal\">",
        "<signal name=\"Changed\">",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    assert_eq!(xml.matches("<method ").count(), 6);
    assert_eq!(xml.matches("<signal ").count(), 1);
    assert_eq!(xml.matches("<property ").count(), 0);
    // The two keys are the only descriptors; no string in the interface carries one.
    assert_eq!(
        xml.matches("<arg type=\"h\" direction=\"out\"/>").count(),
        2
    );
    assert!(xml.contains("<signal name=\"Changed\">\n   </signal>"));
    // Additive: its own file, and Accounts1's is as it was.
    assert!(
        !porter_dbus::accounts::ACCOUNTS
            .introspection()
            .contains("Broker")
    );
}

#[test]
fn grants1_declares_the_grant_calls_and_hands_streams_only_on_descriptors() {
    let xml = porter_dbus::accounts::GRANTS.introspection();
    assert!(xml.contains("<interface name=\"org.quire.Grants1\">"));
    for member in [
        "Candidates",
        "Availability",
        "Grants",
        "RequestGrant",
        "Revoke",
        "OpenAuthenticated",
        "OpenLinked",
        "Accounts",
        "ExtendAccess",
        "AsksOnUse",
    ] {
        assert!(
            xml.contains(&format!("<method name=\"{member}\">")),
            "{member}"
        );
    }
    assert_eq!(xml.matches("<method ").count(), 10);
    assert_eq!(xml.matches("<signal ").count(), 0);
    // A relayed stream is a descriptor, and no member answers a token or a password.
    assert_eq!(xml.matches("type=\"h\" direction=\"out\"").count(), 2);
    assert_eq!(porter_wire::names::GRANTS_PATH, "/org/quire/Grants1");
}
