use super::*;
use test_kit::Scratch;

/// A Spaces store in a directory of `scratch`.
fn store(scratch: &Scratch) -> SpacesStore {
    SpacesStore::File(scratch.join("porter"))
}

fn app(name: &str) -> AppName {
    AppName::parse(name).expect("app")
}

fn name(text: &str) -> SpaceName {
    SpaceName::parse(text).expect("name")
}

fn ids(book: &SpaceBook) -> Vec<&str> {
    book.list().iter().map(|s| s.id.as_str()).collect()
}

const NOW: UnixSeconds = UnixSeconds(1_790_000_000);

#[tokio::test]
async fn a_first_start_writes_an_empty_list_once() {
    let scratch = Scratch::new("spaces");
    let (book, said) = SpaceBook::open(&store(&scratch)).await;
    assert_eq!(said, None);
    assert!(book.list().is_empty());
    assert!(scratch.path().join("porter/spaces.json").exists());
}

#[tokio::test]
async fn ids_are_minted_not_named_and_survive_renames_and_restarts() {
    let scratch = Scratch::new("spaces");
    let (mut book, _) = SpaceBook::open(&store(&scratch)).await;
    let photos = app("org.quire.Photos");
    let at = (Instant::now(), NOW);
    // A registered Space already holds the id the counter would give first.
    let taken = DesktopSpace::parse("space-1").expect("id");
    book.register(&photos, taken, name("Taken"), SpaceLook::default(), at)
        .await
        .expect("registered");
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

    let (after, said) = SpaceBook::open(&store(&scratch)).await;
    assert_eq!(said, None);
    assert_eq!(ids(&after), ["space-1", "space-2", "space-3"]);
    assert_eq!(after.list()[1].name.as_str(), "Office");
    assert_eq!(after.list()[1].look.as_str(), "teal");
    assert!(scratch.path().join("porter/spaces.json.bak").exists());
}

#[tokio::test]
async fn changing_or_removing_a_space_that_is_not_there_is_refused() {
    let (mut book, _) = SpaceBook::open(&SpacesStore::Memory).await;
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
    let (mut book, _) = SpaceBook::open(&SpacesStore::Memory).await;
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
    let scratch = Scratch::new("spaces");
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
    let (mut book, said) = SpaceBook::open(&store(&scratch)).await;
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
    let (mut book, _) = SpaceBook::open(&store(&scratch)).await;
    assert_eq!(
        book.create(&photos, name("x"), SpaceLook::default(), at)
            .await,
        Err(SpaceFault::Unsaved)
    );
    assert_eq!(std::fs::read_to_string(&file).expect("read"), full);
}

#[tokio::test]
async fn an_unreadable_file_is_left_alone_and_nothing_changes() {
    let scratch = Scratch::new("spaces");
    let dir = scratch.path().join("porter");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("spaces.json"), "{ not the spaces").expect("write");
    let (mut book, said) = SpaceBook::open(&store(&scratch)).await;
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

/// D8: on casement the compositor mints each workspace's id and registers it. Registering it
/// again (every start) changes nothing, not even a name the person gave it since; the counter
/// skips a registered id; only a new id counts toward the per-app limit; a restart keeps it.
#[tokio::test]
async fn a_registered_id_is_kept_as_minted_and_registering_it_again_changes_nothing() {
    let scratch = Scratch::new("spaces");
    let (mut book, _) = SpaceBook::open(&store(&scratch)).await;
    let casement = app("org.quire.Casement");
    let at = (Instant::now(), NOW);
    let workspace = DesktopSpace::parse("s-0123456789abcdef0123456789abcdef").expect("id");
    let taken = DesktopSpace::parse("space-1").expect("id");
    for id in [&workspace, &taken] {
        assert_eq!(
            book.register(&casement, id.clone(), name("One"), SpaceLook::default(), at)
                .await,
            Ok(Registered::New)
        );
    }
    book.rename(&workspace, name("Mine"))
        .await
        .expect("renamed");
    // A whole window of starts registers the same workspaces again: all known, none counted.
    for _ in 0..=CREATES_PER_WINDOW {
        assert_eq!(
            book.register(
                &casement,
                workspace.clone(),
                name("One"),
                SpaceLook::default(),
                at
            )
            .await,
            Ok(Registered::Known)
        );
    }
    assert_eq!(
        book.list()[0].name.as_str(),
        "Mine",
        "the person's name stays"
    );
    let made = book
        .create(&casement, name("Two"), SpaceLook::default(), at)
        .await
        .expect("made");
    assert_eq!(
        made.as_str(),
        "space-2",
        "the counter skips a registered id"
    );

    let (after, said) = SpaceBook::open(&store(&scratch)).await;
    assert_eq!(said, None);
    assert_eq!(
        ids(&after),
        ["s-0123456789abcdef0123456789abcdef", "space-1", "space-2"]
    );
}
