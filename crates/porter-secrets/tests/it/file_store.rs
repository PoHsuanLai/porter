//! `FileSecrets` (feature `test-keys`, TEST ONLY): the file's mode, a crash between the temp file
//! and the rename, and writers that race. The shared `Secrets` contract is in `contract.rs`.
#![cfg(unix)]

use porter_core::{AccountId, Credential, SecretKey, SecretPurpose, SecretText};
use porter_secrets::{FileSecrets, FileSecretsError, PutOutcome, Secrets, SecretsError};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("file-store-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn key(account: &str) -> SecretKey {
    SecretKey {
        account: AccountId::parse(account).expect("id"),
        purpose: SecretPurpose::ApiKey,
    }
}

fn api_key(text: &str) -> Credential {
    Credential::ApiKey(SecretText::new(text))
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn items_survive_a_second_store_over_the_same_file_and_the_file_is_0600() {
    let path = scratch("round-trip").join("deep/er/keys.json");
    let one = FileSecrets::open(&path).expect("created with its directories");
    assert_eq!(mode(&path), 0o600);
    one.put(&key("cloud"), &api_key("sk-1")).await.expect("put");
    let two = FileSecrets::open(&path).expect("reopened");
    assert_eq!(two.get(&key("cloud")).await, Ok(api_key("sk-1")));
    assert_eq!(two.get(&key("other")).await, Err(SecretsError::Missing));
    assert_eq!(mode(&path), 0o600, "rewrites keep the mode");
}

#[test]
fn a_relative_path_is_refused() {
    assert_eq!(
        FileSecrets::open("keys.json"),
        Err(FileSecretsError::NotAbsolute)
    );
}

#[tokio::test]
async fn a_file_open_to_group_or_other_is_refused_on_open_and_on_every_use() {
    for bits in [0o640, 0o604, 0o660, 0o666, 0o644, 0o601] {
        let path = scratch("mode").join("keys.json");
        let store = FileSecrets::open(&path).expect("created");
        store
            .put(&key("cloud"), &api_key("sk-1"))
            .await
            .expect("put");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(bits)).expect("chmod");
        assert_eq!(
            FileSecrets::open(&path),
            Err(FileSecretsError::Mode { mode: bits }),
            "{bits:o}"
        );
        // The store opened before the chmod refuses too, and neither reads nor changes the file.
        assert_eq!(
            store.get(&key("cloud")).await,
            Err(SecretsError::Unavailable)
        );
        assert_eq!(
            store.put(&key("x"), &api_key("sk-2")).await,
            Err(SecretsError::Unavailable)
        );
        assert_eq!(mode(&path), bits, "the refusal does not fix the mode");
        assert!(
            !std::fs::read_to_string(&path)
                .expect("read")
                .contains("sk-2")
        );
        // Back to 0600 it is whole again.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert_eq!(store.get(&key("cloud")).await, Ok(api_key("sk-1")));
    }
    // Owner bits other than 0600 are fine (0400 reads; a write is the file system's refusal).
    let path = scratch("mode-owner").join("keys.json");
    let store = FileSecrets::open(&path).expect("created");
    store.put(&key("cloud"), &api_key("k")).await.expect("put");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).expect("chmod");
    assert_eq!(store.get(&key("cloud")).await, Ok(api_key("k")));
}

#[test]
fn a_directory_or_a_file_that_is_not_a_key_file_is_refused() {
    let dir = scratch("kinds");
    assert_eq!(
        FileSecrets::open(dir.join("subdir")).map(|_| ()),
        Ok(()),
        "a missing path is created as a file"
    );
    std::fs::create_dir(dir.join("adir")).expect("dir");
    assert_eq!(
        FileSecrets::open(dir.join("adir")),
        Err(FileSecretsError::NotAFile)
    );
    for (name, text) in [
        ("empty", ""),
        ("garbage", "{ nope"),
        ("old", r#"{"version":0,"items":[]}"#),
        ("noitems", r#"{"version":1}"#),
        ("baditem", r#"{"version":1,"items":[{"account":"a"}]}"#),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert_eq!(
            FileSecrets::open(&path),
            Err(FileSecretsError::Corrupt),
            "{name}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            text,
            "untouched"
        );
    }
}

#[tokio::test]
async fn a_crash_between_the_temp_file_and_the_rename_leaves_the_original_whole() {
    let path = scratch("crash").join("keys.json");
    let store = FileSecrets::open(&path).expect("created");
    store
        .put(&key("cloud"), &api_key("sk-1"))
        .await
        .expect("put");
    let before = std::fs::read(&path).expect("original");

    // The state a crash leaves: the temp file written (here with a different, half-written
    // content and a loose mode), the rename never done.
    let temp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&temp, br#"{"version":1,"items":[{"half"#).expect("temp");
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert_eq!(
        std::fs::read(&path).expect("original"),
        before,
        "the original is intact"
    );
    let reopened = FileSecrets::open(&path).expect("a stale temp does not stop a reopen");
    assert_eq!(reopened.get(&key("cloud")).await, Ok(api_key("sk-1")));

    // The next write replaces the stale temp, commits, and leaves no temp (nor its loose mode).
    reopened
        .put(&key("more"), &api_key("sk-2"))
        .await
        .expect("put");
    assert!(!temp.exists(), "the temp was renamed over the file");
    assert_eq!(mode(&path), 0o600);
    assert_eq!(reopened.get(&key("cloud")).await, Ok(api_key("sk-1")));
    assert_eq!(reopened.get(&key("more")).await, Ok(api_key("sk-2")));
}

/// Writers are serialised by an advisory lock on `<file>.lock`, across threads and across
/// separate stores (as a daemon and `accountd add` in two processes would be): none loses an
/// update, and a racing `put_if_absent` stores once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_are_serialised_and_none_loses_an_update() {
    let path = scratch("race").join("keys.json");
    let store = Arc::new(FileSecrets::open(&path).expect("created"));
    let mut tasks = Vec::new();
    for n in 0..24 {
        // Half the writers go through their own store over the same path.
        let mine = match n % 2 {
            0 => Arc::clone(&store),
            _ => Arc::new(FileSecrets::open(&path).expect("another store")),
        };
        tasks.push(tokio::spawn(async move {
            mine.put(&key(&format!("acct-{n}")), &api_key(&format!("v{n}")))
                .await
                .expect("put");
            mine.put_if_absent(&key("shared"), &api_key(&format!("w{n}")))
                .await
                .expect("put_if_absent")
        }));
    }
    let mut stored = 0;
    for task in tasks {
        stored += usize::from(task.await.expect("task") == PutOutcome::Stored);
    }
    assert_eq!(stored, 1, "exactly one put_if_absent stored");
    for n in 0..24 {
        assert_eq!(
            store.get(&key(&format!("acct-{n}"))).await,
            Ok(api_key(&format!("v{n}"))),
            "writer {n}"
        );
    }
    assert!(store.get(&key("shared")).await.is_ok());
}

#[tokio::test]
async fn the_file_names_each_item_by_the_keyring_attributes_and_debug_hides_values() {
    let path = scratch("shape").join("keys.json");
    let store = FileSecrets::open(&path).expect("created");
    store
        .put(&key("cloud"), &api_key("sk-TOPSECRET"))
        .await
        .expect("put");
    let text = std::fs::read_to_string(&path).expect("read");
    let document: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(document["version"], 1);
    let item = &document["items"][0];
    assert_eq!(item["service"], "porter");
    assert_eq!(item["account"], "cloud");
    assert_eq!(item["purpose"], r#"{"kind":"api_key"}"#);
    assert!(!format!("{store:?}").contains("sk-TOPSECRET"));
}
