use super::*;
use porter_core::consent::{Decision, GrantKey, GrantScope, Usage};
use porter_core::{
    AccountId, AppId, CapabilityKind, DataClass, GrantId, Isolation, SpaceId, UnixSeconds,
};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "accountd-spaces-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn store(&self) -> SpacesStore {
        SpacesStore::File(self.0.join("porter"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(name: &str) -> AppName {
    AppName::parse(name).expect("app")
}

fn grant(id: &str, space: SpaceScope) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app: AppId {
                name: app("org.quire.Photos"),
                isolation: Isolation::Flatpak,
            },
            account: AccountId::parse("cloud").expect("account"),
            kind: CapabilityKind::Storage,
            class: DataClass::Files,
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

fn name(text: &str) -> SpaceName {
    SpaceName::parse(text).expect("name")
}

fn ids(book: &SpaceBook) -> Vec<&str> {
    book.list().iter().map(|s| s.id.as_str()).collect()
}

const NOW: UnixSeconds = UnixSeconds(1_790_000_000);

#[tokio::test]
async fn a_first_start_adopts_the_slugs_grants_are_scoped_to_once() {
    let scratch = Scratch::new();
    let grants = vec![
        grant("g1", only("work")),
        grant("g2", only("work")),
        grant("g3", only("home")),
        grant("g4", only("desktop")),
        grant("g5", only("app:org.quire.Photos:2")),
        grant("g6", SpaceScope::Any),
    ];
    let (book, said) = SpaceBook::open(&scratch.store(), &grants, NOW).await;
    assert_eq!(said, None);
    assert_eq!(ids(&book), ["home", "work"]);
    let work = &book.list()[1];
    assert_eq!(work.name.as_str(), "work");
    assert_eq!(work.look, SpaceLook::default());
    assert!(scratch.path().join("porter/spaces.json").exists());

    // A second start reads the file and adopts nothing again, even a new slug.
    let more = vec![grant("g7", only("garden"))];
    let (again, _) = SpaceBook::open(&scratch.store(), &more, NOW).await;
    assert_eq!(ids(&again), ["home", "work"]);
}

#[tokio::test]
async fn ids_are_minted_not_named_and_survive_renames_and_restarts() {
    let scratch = Scratch::new();
    // An adopted Space already holds the id the counter would give first.
    let (mut book, _) =
        SpaceBook::open(&scratch.store(), &[grant("g", only("space-1"))], NOW).await;
    let photos = app("org.quire.Photos");
    let at = (Instant::now(), NOW);
    let made = book
        .create(
            &photos,
            name("Work"),
            SpaceLook::parse(r#"{"c":1}"#).expect("look"),
            at,
        )
        .await
        .expect("made");
    assert_eq!(made.as_str(), "space-2");
    let other = book
        .create(&photos, name("Work"), SpaceLook::default(), at)
        .await
        .expect("made");
    assert_eq!(
        other.as_str(),
        "space-3",
        "a second Space of the same name is its own"
    );
    book.rename(&made, name("Office")).await.expect("renamed");
    book.set_look(&made, SpaceLook::parse("teal").expect("look"))
        .await
        .expect("look");

    let (after, said) = SpaceBook::open(&scratch.store(), &[], NOW).await;
    assert_eq!(said, None);
    assert_eq!(ids(&after), ["space-1", "space-2", "space-3"]);
    assert_eq!(after.list()[1].name.as_str(), "Office");
    assert_eq!(after.list()[1].look.as_str(), "teal");
    assert!(scratch.path().join("porter/spaces.json.bak").exists());
}

#[tokio::test]
async fn changing_or_removing_a_space_that_is_not_there_is_refused() {
    let (mut book, _) = SpaceBook::open(&SpacesStore::Memory, &[], NOW).await;
    let ghost = DesktopSpace::parse("ghost").expect("id");
    assert_eq!(
        book.rename(&ghost, name("x")).await,
        Err(SpaceFault::NotThere)
    );
    assert_eq!(
        book.set_look(&ghost, SpaceLook::default()).await,
        Err(SpaceFault::NotThere)
    );
    assert_eq!(book.remove(&ghost).await, Err(SpaceFault::NotThere));
}

#[tokio::test]
async fn creates_are_limited_per_app_within_the_window() {
    let (mut book, _) = SpaceBook::open(&SpacesStore::Memory, &[], NOW).await;
    let (flood, calm) = (app("org.example.Flood"), app("org.example.Calm"));
    let start = Instant::now();
    for n in 0..CREATES_PER_WINDOW {
        book.create(&flood, name("x"), SpaceLook::default(), (start, NOW))
            .await
            .unwrap_or_else(|e| panic!("create {n}: {e:?}"));
    }
    let late = start + Duration::from_secs(59);
    assert_eq!(
        book.create(&flood, name("x"), SpaceLook::default(), (late, NOW))
            .await,
        Err(SpaceFault::TooMany)
    );
    // Another app is not held back by it.
    assert!(
        book.create(&calm, name("y"), SpaceLook::default(), (late, NOW))
            .await
            .is_ok()
    );
    // Once the window has passed the app may make Spaces again.
    let later = start + CREATE_WINDOW;
    assert!(
        book.create(&flood, name("x"), SpaceLook::default(), (later, NOW))
            .await
            .is_ok()
    );
    assert_eq!(book.list().len(), CREATES_PER_WINDOW + 2);
}

/// idiom-11: the counter is read from a file, so a file whose counter is at the top of its range
/// must be refused as a full counter. Before, `n + 1` overflowed (a panic in a debug build, a
/// wrap to 0 in a release build, which would mint `space-0` and then reuse every id).
#[tokio::test]
async fn a_counter_at_the_top_of_its_range_is_refused_and_not_wrapped() {
    let scratch = Scratch::new();
    let dir = scratch.path().join("porter");
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("spaces.json");
    let at = (Instant::now(), NOW);
    let photos = app("org.quire.Photos");

    // The last id the counter can mint is made; the counter cannot move past it.
    let almost = format!(
        r#"{{"version":{VERSION},"next":{},"spaces":[]}}"#,
        u64::MAX - 1
    );
    std::fs::write(&file, &almost).expect("write");
    let (mut book, said) = SpaceBook::open(&scratch.store(), &[], NOW).await;
    assert_eq!(said, None);
    let made = book
        .create(&photos, name("Last"), SpaceLook::default(), at)
        .await
        .expect("the second to last number is still there");
    assert_eq!(made.as_str(), format!("space-{}", u64::MAX - 1));
    assert_eq!(
        book.create(&photos, name("More"), SpaceLook::default(), at)
            .await,
        Err(SpaceFault::Unsaved)
    );
    assert_eq!(book.list().len(), 1, "the refused create changed nothing");

    // A file that starts at the top: refused outright.
    let full = format!(r#"{{"version":{VERSION},"next":{},"spaces":[]}}"#, u64::MAX);
    std::fs::write(&file, &full).expect("write");
    let (mut book, _) = SpaceBook::open(&scratch.store(), &[], NOW).await;
    assert_eq!(
        book.create(&photos, name("x"), SpaceLook::default(), at)
            .await,
        Err(SpaceFault::Unsaved)
    );
    assert_eq!(std::fs::read_to_string(&file).expect("read"), full);
}

#[tokio::test]
async fn an_unreadable_file_is_left_alone_and_nothing_changes() {
    let scratch = Scratch::new();
    let dir = scratch.path().join("porter");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("spaces.json"), "{ not the spaces").expect("write");
    let (mut book, said) =
        SpaceBook::open(&scratch.store(), &[grant("g", only("work"))], NOW).await;
    assert!(said.is_some_and(|s| s.contains("spaces.json")));
    assert!(book.list().is_empty());
    assert_eq!(
        book.create(
            &app("org.quire.Photos"),
            name("x"),
            SpaceLook::default(),
            (Instant::now(), NOW)
        )
        .await,
        Err(SpaceFault::Unsaved)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("spaces.json")).expect("read"),
        "{ not the spaces"
    );
}
