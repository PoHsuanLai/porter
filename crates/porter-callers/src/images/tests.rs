use super::*;

#[test]
fn a_daemon_is_installed_under_each_root_and_a_program_in_bin() {
    assert_eq!(
        installed(Install::Libexec, "inferd"),
        [
            "/usr/libexec/quire/inferd",
            "/usr/local/libexec/quire/inferd",
            "/opt/quire/releases/*/libexec/quire/inferd",
        ]
    );
    assert_eq!(installed(Install::Bin, "sill")[0], "/usr/bin/sill");
}

#[test]
fn only_a_plain_service_name_is_a_daemon() {
    let cases = [
        ("inferd.service", Some("inferd")),
        ("docket-acp.service", Some("docket-acp")),
        ("org.quire.Settings.service", None),
        ("sill-shell.scope", None),
        (".service", None),
        ("Inferd.service", None),
        ("a/b.service", None),
    ];
    for (unit, want) in cases {
        assert_eq!(daemon_name(unit), want, "{unit}");
    }
}
