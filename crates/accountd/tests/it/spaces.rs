//! `org.quire.Spaces1` (lane spaces): the desktop-wide Spaces accountd keeps. Who may call what,
//! the `Changed` signal, a removal ending the grants scoped to the Space, the legacy ids adopted
//! once, the per-app limit on new Spaces, and a restart keeping them.

use crate::common;

use accountd::{CREATES_PER_WINDOW, Options, SpacesStore};
use common::*;
use porter_core::audit::AuditEvent;
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    CapabilityKind, DataClass, DesktopSpace, GrantId, SpaceId, SpaceScope, UnixSeconds,
};
use porter_dbus::{CallerRole, SpaceChangedStream, SpacesProxy};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A scratch state directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "accountd-it-spaces-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn options(&self) -> Options {
        Options {
            spaces: SpacesStore::File(self.0.join("porter")),
            ..Options::default()
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn grant(id: &str, space: SpaceScope) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app: photos(),
            account: storage_account().id,
            kind: CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            space,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

fn only(space: &str) -> SpaceScope {
    SpaceScope::Only(SpaceId::parse(space).expect("space"))
}

async fn start(options: Options, grants: Vec<Grant>) -> Rig {
    Rig::start_granted(
        options,
        SheetHost::quiet(),
        vec![porter_fake::cloud_provider()],
        vec![storage_account()],
        grants,
    )
    .await
}

async fn spaces(connection: &zbus::Connection) -> SpacesProxy<'static> {
    SpacesProxy::new(connection).await.expect("proxy")
}

/// The ids and names `List` answers.
async fn listed(proxy: &SpacesProxy<'_>) -> Vec<(String, String)> {
    proxy
        .list()
        .await
        .expect("list")
        .into_iter()
        .map(|(id, details)| (id, text_of(&details, "name").unwrap_or_default()))
        .collect()
}

fn settings_caller() -> porter_dbus::Caller {
    caller("org.quire.Settings", CallerRole::Settings)
}

/// The next `Changed` within two seconds, as (id, what).
async fn next_change(stream: &mut SpaceChangedStream) -> Option<(String, String)> {
    use zbus::export::futures_core::Stream;
    let next = tokio::time::timeout(
        Duration::from_secs(2),
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)),
    )
    .await
    .ok()??;
    let args = next.args().ok()?;
    Some((args.id.to_string(), args.what.to_string()))
}

#[tokio::test(flavor = "multi_thread")]
async fn any_identified_app_lists_and_creates_and_a_stranger_is_denied() {
    let rig = start(Options::default(), vec![]).await;
    let app = spaces(&rig.client("org.quire.Photos").await).await;
    assert!(listed(&app).await.is_empty());
    let id = app
        .create("Work", r#"{"colour":"teal"}"#)
        .await
        .expect("create");
    assert!(DesktopSpace::parse(&id).is_ok(), "{id}");
    let list = app.list().await.expect("list");
    assert_eq!(list.len(), 1);
    let (listed_id, details) = &list[0];
    assert_eq!(listed_id, &id);
    assert_eq!(text_of(details, "name").as_deref(), Some("Work"));
    assert_eq!(
        text_of(details, "look").as_deref(),
        Some(r#"{"colour":"teal"}"#),
        "the look comes back as it was given"
    );
    assert!(details.contains_key("created"));
    assert!(rig.audit.entries().iter().any(|e| {
        e.app.as_ref() == Some(&photos())
            && e.event
                == AuditEvent::SpaceCreated {
                    space: DesktopSpace::parse(&id).expect("id"),
                }
    }));

    let stranger = spaces(&rig.stranger().await).await;
    for error in [
        stranger.list().await.map(|_| ()).expect_err("list"),
        stranger
            .create("x", "")
            .await
            .map(|_| ())
            .expect_err("create"),
    ] {
        assert_eq!(error_name(&error), ACCESS_DENIED);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_settings_and_the_shell_rename_restyle_and_remove() {
    let rig = start(Options::default(), vec![]).await;
    let app = spaces(&rig.client("org.quire.Photos").await).await;
    let id = app.create("Work", "").await.expect("create");
    for error in [
        app.rename(&id, "Mine").await.expect_err("rename"),
        app.set_look(&id, "red").await.expect_err("look"),
        app.remove(&id).await.expect_err("remove"),
    ] {
        assert_eq!(error_name(&error), ACCESS_DENIED);
    }
    let agent = spaces(
        &rig.client_as(caller("org.quire.Companion", CallerRole::Agent))
            .await,
    )
    .await;
    let refused = agent.rename(&id, "Mine").await.expect_err("agent rename");
    assert_eq!(error_name(&refused), ACCESS_DENIED);

    let settings = spaces(&rig.client_as(settings_caller()).await).await;
    settings.rename(&id, "Office").await.expect("rename");
    let shell = spaces(
        &rig.client_as(caller("org.example.Shell", CallerRole::SheetHost))
            .await,
    )
    .await;
    shell.set_look(&id, "red").await.expect("look");
    assert_eq!(listed(&app).await, [(id.clone(), "Office".to_owned())]);
    shell.remove(&id).await.expect("remove");
    assert!(listed(&app).await.is_empty());

    let gone = settings.rename(&id, "Back").await.expect_err("gone");
    assert_eq!(error_name(&gone), "org.freedesktop.DBus.Error.InvalidArgs");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_names_and_looks_are_invalid_args() {
    let rig = start(Options::default(), vec![]).await;
    let app = spaces(&rig.client("org.quire.Photos").await).await;
    let long_look = "x".repeat(1025);
    for (name, look) in [
        ("", ""),
        ("   ", ""),
        ("a\nb", ""),
        ("Work", long_look.as_str()),
    ] {
        let error = app.create(name, look).await.expect_err("refused");
        assert_eq!(
            error_name(&error),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{name:?}"
        );
    }
    let fits = "x".repeat(1024);
    app.create("Work", &fits).await.expect("1024 bytes fit");
}

#[tokio::test(flavor = "multi_thread")]
async fn known_connections_are_told_each_change() {
    let rig = start(Options::default(), vec![]).await;
    let watcher = spaces(&rig.client("org.quire.Mail").await).await;
    let mut changes = watcher.receive_changed().await.expect("subscribe");
    watcher
        .list()
        .await
        .expect("list joins the connections told");
    let maker = spaces(&rig.client("org.quire.Photos").await).await;
    let id = maker.create("Work", "").await.expect("create");
    assert_eq!(
        next_change(&mut changes).await,
        Some((id.clone(), "created".to_owned()))
    );
    let settings = spaces(&rig.client_as(settings_caller()).await).await;
    settings.rename(&id, "Office").await.expect("rename");
    assert_eq!(
        next_change(&mut changes).await,
        Some((id.clone(), "renamed".to_owned()))
    );
    settings.set_look(&id, "red").await.expect("look");
    assert_eq!(
        next_change(&mut changes).await,
        Some((id.clone(), "look".to_owned()))
    );
    settings.remove(&id).await.expect("remove");
    assert_eq!(
        next_change(&mut changes).await,
        Some((id.clone(), "removed".to_owned()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_space_ends_the_grants_scoped_to_it_alone() {
    let grants = vec![
        grant("in-work", only("work")),
        grant("in-home", only("home")),
        grant("anywhere", SpaceScope::Any),
    ];
    let rig = start(Options::default(), grants).await;
    let settings = spaces(&rig.client_as(settings_caller()).await).await;
    settings.remove("work").await.expect("remove");
    let left: Vec<String> = rig
        .service
        .registry()
        .grants
        .iter()
        .map(|g| g.id.to_string())
        .collect();
    assert_eq!(left, ["in-home", "anywhere"]);
    let events: Vec<AuditEvent> = rig.audit.entries().into_iter().map(|e| e.event).collect();
    assert!(events.contains(&AuditEvent::Revoked {
        grant: GrantId::parse("in-work").expect("id")
    }));
    assert!(events.contains(&AuditEvent::SpaceRemoved {
        space: DesktopSpace::parse("work").expect("id")
    }));
    assert_eq!(
        listed(&settings).await,
        [("home".to_owned(), "home".to_owned())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_slugs_are_adopted_on_the_first_start_only() {
    let scratch = Scratch::new();
    let first = start(
        scratch.options(),
        vec![grant("g1", only("work")), grant("g2", only("desktop"))],
    )
    .await;
    let app = spaces(&first.client("org.quire.Photos").await).await;
    assert_eq!(listed(&app).await, [("work".to_owned(), "work".to_owned())]);
    let settings = spaces(&first.client_as(settings_caller()).await).await;
    settings.rename("work", "Office").await.expect("rename");
    drop(first);

    // A second start reads spaces.json: nothing is adopted twice, and a slug a grant names now
    // is not adopted either.
    let second = start(
        scratch.options(),
        vec![grant("g1", only("work")), grant("g3", only("garden"))],
    )
    .await;
    let app = spaces(&second.client("org.quire.Photos").await).await;
    assert_eq!(
        listed(&app).await,
        [("work".to_owned(), "Office".to_owned())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_keeps_the_spaces() {
    let scratch = Scratch::new();
    let first = start(scratch.options(), vec![]).await;
    let app = spaces(&first.client("org.quire.Photos").await).await;
    let work = app.create("Work", "teal").await.expect("create");
    let home = app.create("Home", "").await.expect("create");
    drop(first);

    let second = start(scratch.options(), vec![]).await;
    let app = spaces(&second.client("org.quire.Photos").await).await;
    assert_eq!(
        listed(&app).await,
        [(work, "Work".to_owned()), (home.clone(), "Home".to_owned())]
    );
    let next = app.create("Garden", "").await.expect("create");
    assert_ne!(next, home, "a minted id is never given twice");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_app_making_too_many_spaces_at_once_is_held_back() {
    let rig = start(Options::default(), vec![]).await;
    let flood = spaces(&rig.client("org.example.Flood").await).await;
    for n in 0..CREATES_PER_WINDOW {
        flood
            .create(&format!("Space {n}"), "")
            .await
            .unwrap_or_else(|e| panic!("create {n}: {e}"));
    }
    let error = flood.create("One more", "").await.expect_err("held back");
    assert_eq!(
        error_name(&error),
        "org.freedesktop.DBus.Error.LimitsExceeded"
    );
    assert!(
        error.to_string().contains("Try again in a minute"),
        "{error}"
    );
    // Another app is not held back by it.
    let calm = spaces(&rig.client("org.example.Calm").await).await;
    calm.create("Mine", "").await.expect("another app");
    let made = rig
        .audit
        .entries()
        .into_iter()
        .filter(|e| matches!(e.event, AuditEvent::SpaceCreated { .. }))
        .count();
    assert_eq!(made, CREATES_PER_WINDOW + 1);
}
