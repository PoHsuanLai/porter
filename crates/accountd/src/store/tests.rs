use super::*;
use porter_core::capability::VocabVersion;
use porter_fake::{llm_account, mail_account, storage_account};
use porter_service::Registry;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory under the system temp directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "accountd-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn registry() -> Persisted {
    Registry {
        accounts: vec![storage_account(), mail_account(), llm_account()],
        grants: vec![],
        toggles: vec![],
    }
    .persisted()
}

fn store_in(scratch: &Scratch) -> FileStore {
    // A directory that does not exist yet: the first save makes it.
    FileStore::new(scratch.path().join("porter"))
}

#[tokio::test]
async fn nothing_stored_loads_empty() {
    let scratch = Scratch::new();
    assert_eq!(store_in(&scratch).load().await, Ok(Persisted::empty()));
}

#[tokio::test]
async fn a_saved_registry_loads_back_equal() {
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    store.save(&registry()).await.expect("saved");
    assert_eq!(store.load().await, Ok(registry()));
    assert_eq!(
        store.path(),
        scratch.path().join("porter").join("registry.json")
    );
    let again = Persisted::empty();
    store.save(&again).await.expect("replaced");
    assert_eq!(store.load().await, Ok(again));
}

#[tokio::test]
async fn a_crash_between_write_and_rename_leaves_the_old_file() {
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    store.save(&registry()).await.expect("saved");
    let before = std::fs::read(store.path()).expect("read");
    // The new document is staged and synced, and the process dies before the rename.
    let next = Persisted::empty().to_json().expect("json");
    let stale = store.stage(&next).expect("staged");
    assert_eq!(std::fs::read(store.path()).expect("read"), before);
    assert_eq!(store.load().await, Ok(registry()));
    // The next run saves under a staging name of its own and leaves the stale file alone.
    store.save(&Persisted::empty()).await.expect("saved");
    assert_eq!(store.load().await, Ok(Persisted::empty()));
    assert!(stale.exists());
}

/// rel-1: saves that overlap (the service now serialises them, but the file must not depend on
/// it) each stage under a name of their own, so every one lands whole and the registry file is
/// always one complete document. Before, all shared `registry.json.tmp`: two writers truncated
/// and interleaved one file, and a rename could publish half of each.
#[tokio::test]
async fn overlapping_saves_never_share_a_staging_file() {
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    let names: std::collections::HashSet<PathBuf> = (0..64).map(|_| store.staging()).collect();
    assert_eq!(names.len(), 64);
    let tasks: Vec<_> = (0..16)
        .map(|n| {
            let store = store.clone();
            let state = match n % 2 {
                0 => registry(),
                _ => Persisted::empty(),
            };
            tokio::spawn(async move { store.save(&state).await })
        })
        .collect();
    for task in tasks {
        task.await.expect("joined").expect("saved");
    }
    let loaded = store.load().await.expect("a whole document");
    assert!(loaded == registry() || loaded == Persisted::empty());
    // Nothing is left staged: every save renamed its own file.
    let left: Vec<_> = std::fs::read_dir(scratch.path().join("porter"))
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn a_corrupt_file_is_refused_and_never_overwritten() {
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    std::fs::create_dir_all(scratch.path().join("porter")).expect("dir");
    let garbage: &[u8] = b"{\"vocab\": 3, \"accounts\": [oops";
    std::fs::write(store.path(), garbage).expect("write");
    assert!(matches!(
        store.load().await,
        Err(StoreError::Fault(StoreFault::Unreadable(_)))
    ));
    assert!(matches!(
        store.save(&registry()).await,
        Err(StoreError::Fault(StoreFault::Unreadable(_)))
    ));
    assert_eq!(std::fs::read(store.path()).expect("read"), garbage);
    // Bytes that are not text are refused the same way.
    std::fs::write(store.path(), [0xff, 0xfe, 0x00]).expect("write");
    assert!(matches!(
        store.load().await,
        Err(StoreError::Fault(StoreFault::Unreadable(_)))
    ));
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_unavailable_not_empty() {
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    // A directory where the file should be: reading it fails, and it is not "nothing stored".
    std::fs::create_dir_all(store.path()).expect("dir");
    assert_eq!(store.load().await, Err(StoreError::Unavailable));
}

/// The migration table: one row per vocabulary bump. There is no bump yet (the current version
/// was the first one persisted, and 3 migrates), so the rows are: older than any file ever written has no
/// migration, newer than this build is refused, and the current one loads. A bump adds its row
/// here with the old document as a fixture.
#[tokio::test]
async fn the_vocabulary_of_the_file_decides_whether_it_loads() {
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    store.save(&registry()).await.expect("saved");
    let text = std::fs::read_to_string(store.path()).expect("read");
    let current = format!("\"vocab\": {}", VocabVersion::CURRENT.0);
    assert!(text.contains(&current));
    let with = |v: u16| text.replace(&current, &format!("\"vocab\": {v}"));

    let older = VocabVersion(porter_core::store::FIRST_PERSISTED.0 - 1);
    std::fs::write(store.path(), with(older.0)).expect("write");
    assert_eq!(
        store.load().await,
        Err(StoreError::Fault(StoreFault::NoMigration(older)))
    );
    let newer = VocabVersion(VocabVersion::CURRENT.0 + 1);
    std::fs::write(store.path(), with(newer.0)).expect("write");
    assert_eq!(
        store.load().await,
        Err(StoreError::Fault(StoreFault::FromNewerVocabulary(newer)))
    );
    // Neither refusal was overwritten by a save.
    assert_eq!(
        store.save(&registry()).await,
        Err(StoreError::Fault(StoreFault::FromNewerVocabulary(newer)))
    );
    std::fs::write(store.path(), &text).expect("write");
    assert_eq!(store.load().await, Ok(registry()));
}

#[cfg(unix)]
#[tokio::test]
async fn the_file_is_readable_by_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    let store = store_in(&scratch);
    store.save(&registry()).await.expect("saved");
    store.save(&registry()).await.expect("saved again");
    let mode = std::fs::metadata(store.path())
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
