//! `identity_of` over real scope names, `/proc/<pid>/cgroup` parsing and `ClaimedId`.

use super::*;

/// The sandbox column of `IDENTITIES`, in a form a `const` can hold.
#[derive(Debug, Clone, Copy)]
enum Sandbox {
    Native,
    Flatpak(&'static str),
    Engine(&'static str, &'static str),
}

impl Sandbox {
    fn facts(self) -> SandboxFacts {
        match self {
            Sandbox::Native => SandboxFacts::Native,
            Sandbox::Flatpak(app) => SandboxFacts::Flatpak {
                app: app.to_owned(),
                instance: "3519218".to_owned(),
            },
            Sandbox::Engine(engine, app) => SandboxFacts::Engine {
                engine: engine.to_owned(),
                app: app.to_owned(),
                instance: "7".to_owned(),
            },
        }
    }
}

const APP_SLICE: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice";

/// The name and isolation a row proves, or `None` for unproven.
type Proves = Option<(&'static str, Isolation)>;

/// (case, the leaf below the app slice, sandbox, what it proves).
const IDENTITIES: &[(&str, &str, Sandbox, Proves)] = &[
    (
        "a scope without a launcher tag",
        "app-org.quire.Mail-12.scope",
        Sandbox::Native,
        Some(("org.quire.Mail", Isolation::Unsandboxed)),
    ),
    (
        "sill's launcher scope",
        "app-sill-org.quire.Mail-12.scope",
        Sandbox::Native,
        Some(("org.quire.Mail", Isolation::Unsandboxed)),
    ),
    (
        "another launcher's tag",
        "app-gnome-org.gnome.Terminal-4021.scope",
        Sandbox::Native,
        Some(("org.gnome.Terminal", Isolation::Unsandboxed)),
    ),
    (
        "an escaped dash in the id",
        r"app-sill-com.example.my_app\x2d2-4.scope",
        Sandbox::Native,
        Some(("com.example.my_app-2", Isolation::Unsandboxed)),
    ),
    (
        "a hexadecimal nonce",
        "app-org.quire.Mail-a3f09c.scope",
        Sandbox::Native,
        Some(("org.quire.Mail", Isolation::Unsandboxed)),
    ),
    (
        "a scope whose name is not reverse-DNS (C39)",
        "app-firefox-3.scope",
        Sandbox::Native,
        None,
    ),
    (
        "sill's scope for a non-reverse-DNS id",
        "app-sill-foot-1.scope",
        Sandbox::Native,
        None,
    ),
    (
        "a Flatpak in its scope",
        "app-flatpak-org.mozilla.firefox-9.scope",
        Sandbox::Flatpak("org.mozilla.firefox"),
        Some(("org.mozilla.firefox", Isolation::Flatpak)),
    ),
    (
        "Flatpak's scope name without the sandbox",
        "app-flatpak-org.mozilla.firefox-9.scope",
        Sandbox::Native,
        None,
    ),
    (
        "a Flatpak run without a scope",
        "dbus-broker.service",
        Sandbox::Flatpak("org.mozilla.firefox"),
        Some(("org.mozilla.firefox", Isolation::Flatpak)),
    ),
    (
        "a Flatpak's security context",
        "app-flatpak-org.mozilla.firefox-9.scope",
        Sandbox::Engine("org.flatpak", "org.mozilla.firefox"),
        Some(("org.mozilla.firefox", Isolation::Flatpak)),
    ),
    (
        "another engine's security context, even in a launcher's scope",
        "app-org.quire.Mail-12.scope",
        Sandbox::Engine("org.example.jail", "org.quire.Mail"),
        None,
    ),
    (
        "Flatpak metadata naming no app",
        "app-flatpak-firefox-9.scope",
        Sandbox::Flatpak("firefox"),
        None,
    ),
    (
        "an XDG service unit, not a scope",
        "app-org.quire.Mail@3.service",
        Sandbox::Native,
        None,
    ),
    (
        "a child cgroup below the scope",
        "app-org.quire.Mail-12.scope/worker",
        Sandbox::Native,
        None,
    ),
    (
        "an unescaped dash makes the name ambiguous",
        "app-org.foo-bar-baz-12.scope",
        Sandbox::Native,
        None,
    ),
    (
        "a launcher tag that is not one word",
        "app-org.foo-org.quire.Mail-12.scope",
        Sandbox::Native,
        None,
    ),
    (
        "a broken escape",
        r"app-org.quire.M\xzzail-1.scope",
        Sandbox::Native,
        None,
    ),
    (
        "no nonce",
        "app-org.quire.Mail-.scope",
        Sandbox::Native,
        None,
    ),
];

fn facts(cgroup: &str, sandbox: Sandbox) -> PeerFacts {
    PeerFacts {
        pid: 4242,
        cgroup: CgroupPath::parse(cgroup).expect("cgroup path"),
        sandbox: sandbox.facts(),
    }
}

#[test]
fn identities_follow_scopes_and_sandboxes() {
    for (case, leaf, sandbox, want) in IDENTITIES {
        let got = identity_of(&facts(&format!("{APP_SLICE}/{leaf}"), *sandbox));
        let want = want.map_or(PeerIdentity::Unproven, |(name, isolation)| {
            PeerIdentity::Proven(AppId {
                name: AppName::parse(name).expect("name"),
                isolation,
            })
        });
        assert_eq!(got, want, "{case}");
    }
}

#[test]
fn processes_outside_any_app_scope_are_unproven() {
    for cgroup in [
        "/",
        "/user.slice/user-1000.slice/session-2.scope",
        "/system.slice/sshd.service",
    ] {
        assert_eq!(
            identity_of(&facts(cgroup, Sandbox::Native)),
            PeerIdentity::Unproven,
            "{cgroup}"
        );
    }
}

/// (case, `/proc/<pid>/cgroup` text, the path read or `None`).
const PROC_FILES: &[(&str, &str, Option<&str>)] = &[
    (
        "unified only",
        "0::/user.slice/app-org.quire.Mail-12.scope\n",
        Some("/user.slice/app-org.quire.Mail-12.scope"),
    ),
    (
        "hybrid",
        "12:pids:/user.slice\n1:name=systemd:/user.slice\n0::/app.slice/app-firefox-3.scope\n",
        Some("/app.slice/app-firefox-3.scope"),
    ),
    ("v1 only", "12:pids:/user.slice\n", None),
    ("relative path", "0::user.slice\n", None),
    ("empty", "", None),
];

#[test]
fn the_unified_line_of_proc_cgroup_is_the_path() {
    for (case, contents, want) in PROC_FILES {
        let got = CgroupPath::from_proc_cgroup(contents).ok();
        assert_eq!(got.as_ref().map(CgroupPath::as_str), *want, "{case}");
    }
}

/// (case, text, whether it is a claimed id).
const CLAIMS: &[(&str, &str, bool)] = &[
    ("a bare word", "foot", true),
    ("reverse-DNS", "org.quire.Mail", true),
    ("spaces and punctuation", "Steam Big Picture (beta)", true),
    ("non-ASCII", "café", true),
    ("empty", "", false),
    ("slash", "usr/bin/foot", false),
    ("newline", "foot\n", false),
    ("NUL", "foot\0", false),
];

#[test]
fn claimed_ids_are_printable_text_without_a_slash() {
    for (case, text, ok) in CLAIMS {
        assert_eq!(ClaimedId::parse(text).is_ok(), *ok, "{case}");
    }
    assert!(ClaimedId::parse(&"a".repeat(255)).is_ok());
    assert!(ClaimedId::parse(&"a".repeat(256)).is_err());
}

#[test]
fn claimed_ids_round_trip() {
    for text in ["foot", "Steam Big Picture (beta)", "café"] {
        let id = ClaimedId::parse(text).expect("claimed id");
        assert_eq!(id.as_str(), text);
        let json = serde_json::to_string(&id).expect("json");
        assert_eq!(json, serde_json::to_string(text).expect("json"));
        assert_eq!(serde_json::from_str::<ClaimedId>(&json).expect("id"), id);
    }
    assert!(serde_json::from_str::<ClaimedId>(r#""usr/bin/foot""#).is_err());
}

#[test]
fn identities_and_sandbox_facts_pin_their_json() {
    let mail = PeerIdentity::Proven(AppId {
        name: AppName::parse("org.quire.Mail").expect("name"),
        isolation: Isolation::Unsandboxed,
    });
    let identities = [
        (
            mail,
            r#"{"kind":"proven","v":{"name":"org.quire.Mail","isolation":"unsandboxed"}}"#,
        ),
        (PeerIdentity::Unproven, r#"{"kind":"unproven"}"#),
    ];
    for (identity, json) in identities {
        assert_eq!(serde_json::to_string(&identity).expect("json"), json);
        assert_eq!(
            serde_json::from_str::<PeerIdentity>(json).expect("identity"),
            identity
        );
    }
    let sandboxes = [
        (Sandbox::Native, r#"{"kind":"native"}"#),
        (
            Sandbox::Flatpak("org.mozilla.firefox"),
            r#"{"kind":"flatpak","v":{"app":"org.mozilla.firefox","instance":"3519218"}}"#,
        ),
        (
            Sandbox::Engine("org.flatpak", "org.mozilla.firefox"),
            r#"{"kind":"engine","v":{"engine":"org.flatpak","app":"org.mozilla.firefox","instance":"7"}}"#,
        ),
    ];
    for (sandbox, json) in sandboxes {
        let facts = sandbox.facts();
        assert_eq!(serde_json::to_string(&facts).expect("json"), json);
        assert_eq!(
            serde_json::from_str::<SandboxFacts>(json).expect("facts"),
            facts
        );
    }
}
