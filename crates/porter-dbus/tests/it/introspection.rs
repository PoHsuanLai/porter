//! The checked-in introspection files are the interfaces the skeletons declare. A change to a
//! signature changes the file in the same commit.

use porter_dbus::{Bus, introspection};
use std::path::PathBuf;

#[test]
fn checked_in_introspection_matches_the_interfaces() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dbus");
    for bus in [
        Bus::Accounts,
        Bus::AccountsSheet,
        Bus::Sync,
        Bus::Inference,
        Bus::InferenceAgents,
        Bus::Spaces,
        Bus::Tailnet,
    ] {
        let path = dir.join(bus.file_name());
        let expected =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let actual = match bus {
            Bus::Sync => without_picker(&introspection(bus)),
            _ => introspection(bus),
        };
        assert!(
            actual == expected,
            "{} differs from the interfaces; it should read:\n{actual}",
            path.display()
        );
    }
}

/// The Sync1 document without the Photos Picker, which is outside the default surface (the
/// `photos-picker` feature adds it, and the test below it checks it then).
fn without_picker(xml: &str) -> String {
    match xml.split_once("<interface name=\"org.quire.Photos1.Picker\">") {
        Some((sync1, _)) => format!("{}</node>\n", sync1.trim_end_matches(' ')),
        None => xml.to_owned(),
    }
}

#[test]
fn the_spaces_registry_declares_its_five_methods_and_one_signal() {
    let xml = introspection(Bus::Spaces);
    for member in [
        "<method name=\"List\">",
        "<arg type=\"a(sa{sv})\" direction=\"out\"/>",
        "<method name=\"Create\">",
        "<method name=\"Rename\">",
        "<method name=\"SetLook\">",
        "<method name=\"Remove\">",
        "<signal name=\"Changed\">",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    assert_eq!(xml.matches("<method ").count(), 5);
    assert_eq!(xml.matches("<signal ").count(), 1);
    assert_eq!(porter_dbus::SPACES_PATH, "/org/quire/Spaces1");
    assert!(
        !introspection(Bus::Accounts).contains("org.quire.Spaces1"),
        "its own file, beside Accounts1's"
    );
}

#[test]
fn the_tailnet_object_declares_machines_and_changed_and_nothing_else() {
    let xml = introspection(Bus::Tailnet);
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
    assert_eq!(porter_dbus::TAILNET_PATH, "/org/quire/Tailnet1");
    // Additive: its own file, and Accounts1's and Spaces1's are as they were.
    assert!(!introspection(Bus::Accounts).contains("Tailnet1"));
    assert!(!introspection(Bus::Spaces).contains("Tailnet1"));
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
        "<method name=\"IssueProcessCredential\">",
        "<method name=\"RevokeProcessCredential\">",
        "<signal name=\"ProcessCredentialRevoked\">",
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
    // `Places` was added after the freeze: an additive method (no existing member, argument or
    // error changes), as `Sync1.ConfirmDiscard` was.
    assert!(xml.contains("<method name=\"Places\">"));
    assert!(xml.contains("<arg type=\"a(sa{sv})\" direction=\"out\"/>"));
    // So were `AddComputer` and `RemoveComputer`, for Settings.
    assert!(xml.contains("<method name=\"AddComputer\">"));
    assert!(xml.contains("<method name=\"RemoveComputer\">"));
    assert!(xml.contains("<arg name=\"models\" type=\"a(sa{sv})\" direction=\"in\"/>"));
    // And these, for lending models to the person's other computers over their Tailscale
    // network (all additive): the computers that could be added, adding one, and the computers
    // that ask to use this one (who is asking, the answer, taking an answer back).
    for member in [
        "<method name=\"Candidates\">",
        "<method name=\"AddTailnetComputer\">",
        "<method name=\"Guests\">",
        "<method name=\"AnswerGuest\">",
        "<method name=\"ForgetGuest\">",
        "<signal name=\"GuestAsks\">",
        "<signal name=\"GuestsChanged\">",
        "<arg name=\"answer\" type=\"s\" direction=\"in\"/>",
    ] {
        assert!(xml.contains(member), "missing {member}");
    }
    let declared = xml.matches("<method ").count()
        + xml.matches("<signal ").count()
        + xml.matches("<property ").count();
    assert_eq!(
        declared,
        members.len() + 3 + 7,
        "no member beyond the frozen seven, Places, AddComputer, RemoveComputer, and the five \
         methods and two signals of lending models across the person's network"
    );
    assert_eq!(
        porter_dbus::COMPUTER_ERROR_PREFIX,
        "org.quire.Inference1.Error.Computer."
    );
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
fn the_peer_interface_is_the_daemon_members_and_the_launchers() {
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
        "<method name=\"RegisterLauncher\">",
        "<method name=\"BeginSession\">",
        "<method name=\"EndSession\">",
        "<method name=\"RequestAgentGrant\">",
        "<method name=\"ReportAgentLogin\">",
        "<method name=\"ReportAgentLogout\">",
        "<signal name=\"AgentLoginRequested\">",
        "<signal name=\"AgentLogoutRequested\">",
    ] {
        assert!(peer.contains(member), "missing {member}");
    }
    assert_eq!(peer.matches("<method ").count(), 10);
    assert_eq!(peer.matches("<signal ").count(), 2);
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
    let sync1 = without_picker(&sync);
    assert_eq!(sync1.matches("<method ").count(), 8);
    assert_eq!(sync1.matches("<signal ").count(), 3);
    assert!(sync1.contains("<method name=\"ConfirmDiscard\">"));
    assert!(sync1.contains("<method name=\"SyncNow\">"));
    assert_eq!(
        porter_dbus::SYNC_ERROR_PAUSED,
        format!("{}Paused", porter_dbus::SYNC_ERROR_PREFIX)
    );
    assert!(sync1.contains("<method name=\"Watch\">"));
    assert!(sync1.contains("<signal name=\"NeedsConfirmation\">"));
    assert_eq!(
        porter_dbus::SYNC_ERROR_NOTHING_HELD,
        format!("{}NothingHeld", porter_dbus::SYNC_ERROR_PREFIX)
    );
    #[cfg(feature = "photos-picker")]
    {
        let (_, picker) = sync
            .split_once("<interface name=\"org.quire.Photos1.Picker\">")
            .expect("the Picker interface is declared beside Sync1");
        for member in ["Start", "Poll", "Import", "Cancel"] {
            assert!(
                picker.contains(&format!("<method name=\"{member}\">")),
                "{member}"
            );
        }
        assert_eq!(picker.matches("<method ").count(), 4);
        assert_eq!(picker.matches("<signal ").count(), 0);
        assert!(
            picker.contains("<arg type=\"as\" direction=\"out\"/>"),
            "Import answers the files' paths"
        );
        assert_eq!(
            porter_dbus::PICKER_ERROR_NOT_YET,
            format!("{}NotYet", porter_dbus::PICKER_ERROR_PREFIX)
        );
    }
    #[cfg(not(feature = "photos-picker"))]
    assert!(
        !sync.contains("org.quire.Photos1"),
        "the Picker is not declared by default"
    );
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
