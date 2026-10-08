use super::*;

fn app(name: &str) -> AppName {
    AppName::parse(name).expect("app name")
}

fn id(text: &str) -> SpaceId {
    SpaceId::parse(text).expect("space id")
}

#[test]
fn the_reserved_desktop_space_is_a_valid_id() {
    assert_eq!(SpaceId::parse("desktop"), Ok(SpaceId::desktop()));
    assert_eq!(SpaceId::desktop().kind(), SpaceKind::Outside);
    assert!(SpaceId::parse("Work Space").is_err());
}

#[test]
fn every_kind_round_trips_through_its_text() {
    let photos = app("org.quire.Photos");
    let work = DesktopSpace::parse("work").expect("slug");
    let cases = [
        (SpaceId::desktop(), "desktop", SpaceKind::Outside),
        (
            SpaceId::linked(&work),
            "work",
            SpaceKind::Linked(work.clone()),
        ),
        (
            SpaceId::app(&photos, LocalSpace(7)),
            "app:org.quire.Photos:7",
            SpaceKind::App {
                app: photos.clone(),
                local: LocalSpace(7),
            },
        ),
        (
            SpaceId::app(&app("com.example.my_app-2"), LocalSpace(u64::MAX)),
            "app:com.example.my_app-2:18446744073709551615",
            SpaceKind::App {
                app: app("com.example.my_app-2"),
                local: LocalSpace(u64::MAX),
            },
        ),
    ];
    for (space, text, kind) in cases {
        assert_eq!(space.as_str(), text);
        assert_eq!(space.to_string(), text);
        assert_eq!(id(text), space, "{text}");
        assert_eq!(space.kind(), kind, "{text}");
        let json = serde_json::to_string(&space).expect("json");
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<SpaceId>(&json).expect("back"), space);
    }
}

#[test]
fn ids_stored_before_the_kinds_read_as_desktop_wide_spaces() {
    for old in [
        "work",
        "home",
        "space-3",
        "67e55044-10b1-426f-9247-bb680e5fe0c8",
    ] {
        let space: SpaceId = serde_json::from_str(&format!("\"{old}\"")).expect("old id");
        assert_eq!(
            space.kind(),
            SpaceKind::Linked(DesktopSpace::parse(old).expect("slug")),
            "{old}"
        );
        assert_eq!(space.owner(), None);
    }
}

#[test]
fn texts_that_are_neither_form_are_refused() {
    let cases = [
        ("no app element", "app::3"),
        ("one-element app", "app:photos:3"),
        ("no number", "app:org.quire.Photos:"),
        ("not a number", "app:org.quire.Photos:x"),
        ("leading zero", "app:org.quire.Photos:07"),
        ("sign", "app:org.quire.Photos:+7"),
        ("negative", "app:org.quire.Photos:-7"),
        ("too large", "app:org.quire.Photos:18446744073709551616"),
        ("no number part", "app:org.quire.Photos"),
        ("other prefix", "apps:org.quire.Photos:3"),
        ("upper prefix", "APP:org.quire.Photos:3"),
        ("colon in slug", "work:3"),
        ("empty", ""),
        ("upper slug", "Work"),
    ];
    for (name, text) in cases {
        assert!(SpaceId::parse(text).is_err(), "{name}: {text}");
        assert!(
            serde_json::from_str::<SpaceId>(&format!("\"{text}\"")).is_err(),
            "{name}"
        );
    }
}

#[test]
fn an_app_form_never_reads_as_a_slug_nor_a_slug_as_the_app_form() {
    // The app form holds `:`, which no slug may; so no text is both.
    let space = SpaceId::app(&app("org.quire.Photos"), LocalSpace(1));
    assert!(DesktopSpace::parse(space.as_str()).is_err());
    assert!(!crate::id::is_id(space.as_str()));
    // A slug that starts like the prefix is still a slug.
    assert!(matches!(id("app").kind(), SpaceKind::Linked(_)));
    assert!(matches!(id("app.photos-1").kind(), SpaceKind::Linked(_)));
}

#[test]
fn desktop_is_not_a_desktop_wide_space() {
    assert!(DesktopSpace::parse("desktop").is_err());
    assert!(DesktopSpace::parse("work").is_ok());
}

#[test]
fn for_app_links_or_keeps_the_apps_own_space() {
    let photos = app("org.quire.Photos");
    let linked = SpaceId::for_app(&photos, LocalSpace(4), Some("work")).expect("linked");
    assert_eq!(
        linked.kind(),
        SpaceKind::Linked(DesktopSpace::parse("work").expect("slug"))
    );
    let own = SpaceId::for_app(&photos, LocalSpace(4), None).expect("own");
    assert_eq!(own.as_str(), "app:org.quire.Photos:4");
    assert_eq!(own.owner(), Some(photos.clone()));
    for bad in ["Work", "", "desktop", "app:org.quire.Photos:4", "a b"] {
        assert!(
            SpaceId::for_app(&photos, LocalSpace(4), Some(bad)).is_err(),
            "a bad link is an error, never the app's own Space: {bad:?}"
        );
    }
}

#[test]
fn scopes_pin_their_json() {
    let cases = [
        (SpaceScope::Any, r#"{"kind":"any"}"#),
        (
            SpaceScope::Only(id("work")),
            r#"{"kind":"only","v":"work"}"#,
        ),
        (
            SpaceScope::Only(id("app:org.quire.Photos:2")),
            r#"{"kind":"only","v":"app:org.quire.Photos:2"}"#,
        ),
    ];
    for (scope, json) in cases {
        assert_eq!(serde_json::to_string(&scope).expect("json"), json);
        assert_eq!(
            serde_json::from_str::<SpaceScope>(json).expect("scope"),
            scope
        );
    }
}
