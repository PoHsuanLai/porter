//! The keys inferd held, coming into accountd when it starts: a `test-keys` file in the format
//! inferd's key variable named, and the daemon on a private bus putting the copy off while an
//! inferd that keeps its own keys is running. No real keyring, bus or state is touched; the keys
//! are fakes.
#![cfg(feature = "test-keys")]

use crate::common::bus::PrivateBus;

use accountd::import::{Import, OldStore, Outcome};
use accountd::paths::Paths;
use accountd::{Config, Daemon};
use porter_core::store::{Persisted, RegistryStore};
use porter_core::{AccountId, AuthKind, Credential, SecretKey, SecretPurpose, SecretText};
use porter_dbus::BusTarget;
use porter_secrets::keysel::{Chosen, Selection, select_for};
use porter_secrets::{Namespace, Secrets, SecretsError};
use porter_store::{FileStore, ImportMarker};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use test_kit::GENEROUS;

const KEY: &str = "sk-fake-0001-NOT-A-REAL-KEY";

/// A key file exactly as inferd's `FileSecrets` wrote it: one item of the namespace
/// `porter.inferd`, the account, the purpose in its serde form and the credential.
const INFERDS_FILE: &str = r#"{
  "version": 1,
  "items": [
    {
      "service": "porter.inferd",
      "account": "openrouter",
      "purpose": "{\"kind\":\"api_key\"}",
      "credential": { "kind": "api_key", "v": "sk-fake-0001-NOT-A-REAL-KEY" }
    }
  ]
}"#;

fn namespace(name: &str) -> Namespace {
    Namespace::parse(name).expect("namespace")
}

fn account() -> porter_core::Account {
    let mut account = porter_fake::llm_account();
    account.id = AccountId::parse("openrouter").expect("id");
    account.auth = AuthKind::ApiKey;
    account
}

fn key() -> SecretKey {
    SecretKey {
        account: account().id,
        purpose: SecretPurpose::ApiKey,
    }
}

/// inferd's old registry, as it left it, in `dir/state/quire/inferd/keys`.
async fn seed_registry(state: &Path) {
    let mut persisted = Persisted::empty();
    persisted.accounts.push(account());
    FileStore::new(state.join("quire/inferd/keys"))
        .save(&persisted)
        .await
        .expect("seeded");
}

#[tokio::test]
async fn accountd_keys_reads_the_file_in_the_format_inferds_key_variable_named() {
    let dir = test_kit::Scratch::new("old-keys-format");
    let file = dir.join("inferd.keys");
    std::fs::write(&file, INFERDS_FILE).expect("file");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("mode");
    let selection = select_for(Some(&format!("file:{}", file.display())), true).expect("selected");
    assert!(matches!(selection, Selection::File(_)));
    let old = Chosen::open(selection.clone(), namespace("porter.inferd"), "t")
        .expect("opens")
        .keys;
    let new = Chosen::open(selection, namespace("porter.accountd"), "t")
        .expect("opens")
        .keys;
    assert_eq!(
        old.get(&key()).await.expect("inferd's item reads"),
        Credential::ApiKey(SecretText::new(KEY))
    );
    assert_eq!(
        new.get(&key()).await.map(drop),
        Err(SecretsError::Missing),
        "accountd's own namespace holds nothing yet"
    );

    seed_registry(&dir.join("state")).await;
    let store = FileStore::new(dir.join("state/porter"));
    let old_store = OldStore {
        registry_dir: dir.join("state/quire/inferd/keys"),
        audit: dir.join("state/quire/inferd/keys-audit.jsonl"),
    };
    let (audit, marker) = (dir.join("audit.jsonl"), dir.join("ai-import.json"));
    let outcome = Import {
        old: &old_store,
        old_secrets: &old,
        secrets: &new,
        store: &store,
        audit: &audit,
        marker: &marker,
    }
    .run()
    .await
    .expect("imported");
    assert!(matches!(outcome, Outcome::Imported(_)), "{outcome:?}");
    assert_eq!(
        new.get(&key()).await.expect("accountd's item"),
        Credential::ApiKey(SecretText::new(KEY))
    );
    assert_eq!(old.get(&key()).await.map(drop), Err(SecretsError::Missing));
    // The file is still a key file, and the new item is accountd's.
    let text = std::fs::read_to_string(&file).expect("file");
    assert!(text.contains("porter.accountd"), "{text}");
    assert!(ImportMarker::read(&marker).expect("read").is_some());
}

/// The paths for a person whose whole home is `home`.
fn paths(home: &Path) -> Paths {
    let at = |sub: &str| home.join(sub).to_string_lossy().into_owned();
    let map = [
        ("HOME", at("")),
        ("XDG_STATE_HOME", at("state")),
        ("XDG_CONFIG_HOME", at("config")),
        ("XDG_DATA_HOME", at("data")),
        ("XDG_DATA_DIRS", at("share")),
        (
            accountd::paths::TAILSCALE_SOCKET_VAR,
            at("no-tailscale.sock"),
        ),
    ];
    let mut paths = Paths::resolve(
        |name| {
            map.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        },
        &[],
    )
    .expect("paths");
    paths.callers_system = home.join("no-callers.toml");
    paths.clients_shipped = home.join("no-clients.toml");
    paths.provider_dirs = vec![
        home.join("providers-shipped"),
        home.join("providers-person"),
    ];
    paths
}

/// An inferd of before the keys moved: it owns `org.quire.Inference1` and serves `Keys`.
struct OldInferd;

#[zbus::interface(name = "org.quire.Inference1.Keys")]
impl OldInferd {}

async fn registry_of(home: &Path) -> Persisted {
    FileStore::new(home.join("state/porter"))
        .load()
        .await
        .expect("loaded")
}

fn config(home: &Path, bus: &PrivateBus, keys: &Path) -> Config {
    Config::new(paths(home), BusTarget::Address(bus.address().to_owned()))
        .with_keys(Selection::File(PathBuf::from(keys)))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_copy_waits_for_an_inferd_that_keeps_its_own_keys_and_is_made_when_it_is_gone() {
    let bus = PrivateBus::start();
    let home = bus.scratch().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let keys = home.join("keys.json");
    std::fs::write(&keys, INFERDS_FILE).expect("keys");
    std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o600)).expect("mode");
    seed_registry(&home.join("state")).await;

    // The old inferd is on the bus.
    let inferd = bus.connect().await;
    inferd
        .object_server()
        .at("/org/quire/Inference1", OldInferd)
        .await
        .expect("object");
    inferd
        .request_name("org.quire.Inference1")
        .await
        .expect("name");

    let daemon = tokio::time::timeout(GENEROUS, Daemon::build(config(&home, &bus, &keys)))
        .await
        .expect("starts")
        .expect("starts");
    assert!(
        registry_of(&home).await.accounts.is_empty(),
        "nothing was copied while the old inferd ran"
    );
    let (old, _) = {
        let selection = Selection::File(keys.clone());
        (
            Chosen::open(selection, namespace("porter.inferd"), "t")
                .expect("opens")
                .keys,
            (),
        )
    };
    assert!(
        old.get(&key()).await.is_ok(),
        "inferd's key is where it was"
    );

    // It goes: accountd ends, to be started again.
    let running = tokio::spawn(daemon.run(std::future::pending()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!running.is_finished(), "it waits while inferd runs");
    drop(inferd);
    let ended = tokio::time::timeout(GENEROUS, running)
        .await
        .expect("ends once inferd is gone")
        .expect("task");
    assert_eq!(ended, accountd::daemon::RunEnd::KeysReady);

    // Started again, it copies.
    let daemon = tokio::time::timeout(GENEROUS, Daemon::build(config(&home, &bus, &keys)))
        .await
        .expect("starts")
        .expect("starts");
    let loaded = registry_of(&home).await;
    assert_eq!(loaded.accounts.len(), 1);
    assert_eq!(loaded.accounts[0].id.as_str(), "openrouter");
    assert_eq!(old.get(&key()).await.map(drop), Err(SecretsError::Missing));
    assert!(
        ImportMarker::read(&paths(&home).ai_import)
            .expect("read")
            .is_some_and(|marker| marker.secrets_removed)
    );
    drop(daemon);
}
