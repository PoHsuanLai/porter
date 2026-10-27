use super::{FLATPAK_LAUNCHER, ScopeName, escape, unescape};

/// (case, launcher, app, run, the unit name written).
const WRITTEN: &[(&str, Option<&str>, &str, &str, &str)] = &[
    (
        "sill's launch",
        Some("sill"),
        "org.quire.Mail",
        "12",
        "app-sill-org.quire.Mail-12.scope",
    ),
    (
        "no launcher tag",
        None,
        "org.quire.Mail",
        "a3f09c",
        "app-org.quire.Mail-a3f09c.scope",
    ),
    (
        "a dash in the id",
        Some("sill"),
        "com.example.my_app-2",
        "4",
        r"app-sill-com.example.my_app\x2d2-4.scope",
    ),
    (
        "a space, a slash and a leading dot",
        Some("sill"),
        ".my app/x",
        "1",
        r"app-sill-\x2emy\x20app\x2fx-1.scope",
    ),
    (
        "a non-ASCII id",
        None,
        "café",
        "7",
        r"app-caf\xc3\xa9-7.scope",
    ),
    (
        "Flatpak's run",
        Some(FLATPAK_LAUNCHER),
        "org.mozilla.firefox",
        "9",
        "app-flatpak-org.mozilla.firefox-9.scope",
    ),
];

#[test]
fn what_is_written_is_what_is_read_back() {
    for (case, launcher, app, run, unit) in WRITTEN {
        let written = ScopeName::new(*launcher, app, run).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(written.to_string(), *unit, "{case}: written");
        let read = ScopeName::parse(unit).unwrap_or_else(|| panic!("{case}: not read back"));
        assert_eq!(read, written, "{case}: read back");
        assert_eq!(
            (read.launcher(), read.app(), read.run()),
            (*launcher, *app, *run),
            "{case}: parts"
        );
    }
}

#[test]
fn only_an_app_scope_is_read() {
    for (case, unit) in [
        ("a service", "app-org.quire.Mail-12.service"),
        ("not an app's", "run-u7.scope"),
        ("no run", "app-org.quire.Mail.scope"),
        (
            "a launcher that is not a word",
            "app-si.ll-org.quire.Mail-1.scope",
        ),
        ("a run that is not a word", "app-org.quire.Mail-1_2.scope"),
        ("too many parts", "app-a-b-c-d.scope"),
        ("a broken escape", r"app-org\x2.quire-1.scope"),
        ("a lone backslash", r"app-org\quire-1.scope"),
        ("an empty id", "app--1.scope"),
    ] {
        assert_eq!(ScopeName::parse(unit), None, "{case}: {unit}");
    }
}

#[test]
fn a_launcher_or_run_that_would_not_read_back_is_refused() {
    assert!(ScopeName::new(Some("my-launcher"), "org.quire.Mail", "1").is_err());
    assert!(ScopeName::new(None, "org.quire.Mail", "1-2").is_err());
    assert!(ScopeName::new(None, "org.quire.Mail", "").is_err());
    assert!(ScopeName::new(None, "", "1").is_err());
}

#[test]
fn flatpak_is_its_tag_and_a_decimal_run() {
    let flatpak = |unit: &str| ScopeName::parse(unit).is_some_and(|name| name.is_flatpak());
    assert!(flatpak("app-flatpak-org.mozilla.firefox-9.scope"));
    assert!(!flatpak("app-flatpak-org.mozilla.firefox-a3.scope"));
    assert!(!flatpak("app-sill-org.mozilla.firefox-9.scope"));
}

#[test]
fn a_unit_name_component_escapes_the_dash_the_slash_and_a_leading_dot() {
    const CASES: &[(&str, &str)] = &[
        ("org.quire.Photos", "org.quire.Photos"),
        ("org.example.My-App", r"org.example.My\x2dApp"),
        ("a_b:c", "a_b:c"),
        ("a/b", r"a\x2fb"),
        (".hidden", r"\x2ehidden"),
    ];
    for (text, escaped) in CASES {
        assert_eq!(escape(text), *escaped, "{text}");
        assert_eq!(unescape(escaped).as_deref(), Some(*text), "{text}");
    }
}
