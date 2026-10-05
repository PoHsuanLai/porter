use super::*;

fn row(exe: &str, app: &str, role: CallerRole) -> CallerRow {
    CallerRow {
        exe: PathBuf::from(exe),
        app: AppName::parse(app).expect("name"),
        role,
    }
}

fn table() -> CallerTable {
    CallerTable {
        callers: vec![
            row(
                "/usr/libexec/quire/inferd",
                "org.quire.Inference",
                CallerRole::PorterDaemon,
            ),
            row(
                "/usr/bin/detent",
                "org.quire.Settings",
                CallerRole::Settings,
            ),
            row("/usr/libexec/quire/cuad", "org.quire.Cua", CallerRole::Cua),
        ],
    }
}

#[test]
fn an_executable_resolves_to_its_app_and_role() {
    let cases = [
        (
            "/usr/libexec/quire/inferd",
            Some(("org.quire.Inference", CallerRole::PorterDaemon)),
        ),
        (
            "/usr/bin/detent",
            Some(("org.quire.Settings", CallerRole::Settings)),
        ),
        ("/usr/bin/detent (deleted)", None),
        ("/usr/bin/bash", None),
    ];
    for (exe, want) in cases {
        let got = table()
            .resolve(Path::new(exe))
            .map(|c| (c.app.name.to_string(), c.role));
        assert_eq!(got, want.map(|(n, r)| (n.to_owned(), r)), "{exe}");
    }
    assert_eq!(
        table()
            .resolve(Path::new("/usr/bin/detent"))
            .map(|c| c.app.isolation),
        Some(Isolation::Unsandboxed)
    );
}

#[test]
fn a_user_row_replaces_the_system_row_of_the_same_executable() {
    let user = CallerTable {
        callers: vec![row(
            "/usr/bin/detent",
            "org.example.MySettings",
            CallerRole::App,
        )],
    };
    let merged = CallerTable::layered(table(), user);
    let got = merged.resolve(Path::new("/usr/bin/detent")).expect("named");
    assert_eq!(
        (got.app.name.as_str(), got.role),
        ("org.example.MySettings", CallerRole::App)
    );
    assert_eq!(merged.callers.len(), 3);
}

#[test]
fn a_table_round_trips_through_its_serde_form() {
    let json = serde_json::to_string(&table()).expect("json");
    assert_eq!(
        serde_json::from_str::<CallerTable>(&json).expect("table"),
        table()
    );
    assert!(json.contains(r#""role":"porter_daemon""#));
}
