use super::*;
use crate::CallerRow;
use peephole_core::{ExePath, Pin, RoleName, WhyNot};

fn key(text: &str) -> AppKey {
    AppKey::new(text).expect("key")
}

fn service(app: &str, role: &str, proof: Proof) -> Standing {
    Standing::Service {
        app: key(app),
        role: RoleName::new(role).expect("role"),
        proof,
        pin: Pin::AtConnect,
    }
}

fn table() -> CallerTable {
    let row = |app: &str, role| CallerRow {
        app: AppName::parse(app).expect("name"),
        unit: None,
        role,
        exe: Vec::new(),
        name: None,
    };
    CallerTable {
        callers: vec![
            row("org.example.Sheets", CallerRole::SheetHost),
            row("org.quire.Settings", CallerRole::Settings),
        ],
    }
}

fn shown(standing: &Standing) -> Option<(String, Isolation, CallerRole)> {
    caller_of(standing, &table()).map(|c| (c.app.name.to_string(), c.app.isolation, c.role))
}

fn want(
    app: &str,
    isolation: Isolation,
    role: CallerRole,
) -> Option<(String, Isolation, CallerRole)> {
    Some((app.to_owned(), isolation, role))
}

#[test]
fn a_fixed_role_service_with_full_proof_keeps_its_role() {
    let cases = [
        (
            "org.quire.Inference",
            "porter_daemon",
            CallerRole::PorterDaemon,
        ),
        ("org.quire.Shell", "sheet_host", CallerRole::SheetHost),
        ("org.quire.Cua", "cua", CallerRole::Cua),
        ("org.quire.Settings", "settings", CallerRole::Settings),
    ];
    for (app, role, expected) in cases {
        assert_eq!(
            shown(&service(app, role, Proof::Full)),
            want(app, Isolation::Unsandboxed, expected),
            "{app}"
        );
    }
}

#[test]
fn a_fixed_role_service_with_only_its_program_or_its_cgroup_is_a_plain_app() {
    for proof in [Proof::Exe, Proof::Cgroup] {
        assert_eq!(
            shown(&service("org.quire.Inference", "porter_daemon", proof)),
            want(
                "org.quire.Inference",
                Isolation::Unsandboxed,
                CallerRole::App
            ),
            "{proof:?}"
        );
    }
}

#[test]
fn a_role_that_is_no_more_than_an_app_needs_no_more_than_the_row() {
    assert_eq!(
        shown(&service("org.quire.Intents", "agent", Proof::Cgroup)),
        want(
            "org.quire.Intents",
            Isolation::Unsandboxed,
            CallerRole::Agent
        )
    );
}

#[test]
fn a_role_no_porter_caller_has_is_nobody() {
    assert_eq!(
        shown(&service("org.freedesktop.systemd1", "manager", Proof::Exe)),
        None
    );
}

#[test]
fn a_sandbox_is_its_app_with_the_role_of_the_apps_row() {
    let sandboxed = |app: &str, engine| Standing::Sandboxed {
        app: key(app),
        engine,
    };
    assert_eq!(
        shown(&sandboxed("org.example.Photos", Engine::Flatpak)),
        want("org.example.Photos", Isolation::Flatpak, CallerRole::App)
    );
    assert_eq!(
        shown(&sandboxed("org.example.Sheets", Engine::Flatpak)),
        want(
            "org.example.Sheets",
            Isolation::Flatpak,
            CallerRole::SheetHost
        )
    );
    assert_eq!(
        shown(&sandboxed("org.mozilla.firefox", Engine::Snap)),
        want("org.mozilla.firefox", Isolation::Snap, CallerRole::App)
    );
    // A snap's bare name is not an app name.
    assert_eq!(shown(&sandboxed("firefox", Engine::Snap)), None);
}

#[test]
fn a_program_is_an_app_only_by_the_scope_a_launcher_gave_it() {
    let program = |scope_app: Option<&str>| Standing::Program {
        exe: ExePath::new("/usr/bin/thing").expect("path"),
        scope_app: scope_app.map(key),
    };
    assert_eq!(
        shown(&program(Some("org.example.Mail"))),
        want("org.example.Mail", Isolation::Unsandboxed, CallerRole::App)
    );
    // The role is the table's, by the app's name.
    assert_eq!(
        shown(&program(Some("org.quire.Settings"))),
        want(
            "org.quire.Settings",
            Isolation::Unsandboxed,
            CallerRole::Settings
        )
    );
    assert_eq!(shown(&program(None)), None);
    assert_eq!(shown(&program(Some("firefox"))), None);
}

#[test]
fn an_unverified_process_is_nobody() {
    let unverified = Standing::Unverified {
        exe_name: Some("app".to_owned()),
        why: WhyNot::ImageWritable,
    };
    assert_eq!(identity_of(&unverified), PeerIdentity::Unproven);
    assert_eq!(shown(&unverified), None);
}
