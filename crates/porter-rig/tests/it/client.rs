//! `porter-rig-client` as a process, on a private bus with the real accountd service (a fake
//! provider with one signed-in identity) and syncd's `Sync1` served over a hub. The daemons name
//! the client from the identity fixture the client writes under a proc root (`fixture`), the way
//! a scenario's `test-proc-root` daemons do: the client is `org.example.Rig`, a Flatpak app, and
//! nothing else says so.

use crate::common;

use accountd::{BusSheets, Options, serve_with};
use common::bus::PrivateBus;
use porter_core::clock::FixedClock;
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppName, AuthKind, Claim, Credential, Offer,
    Provenance, Restriction, SecretKey, SecretPurpose, SecretText, Subject, UnixSeconds,
};
use porter_dbus::callers::{CallerTable, ProcCallers};
use porter_fake::{FakeProvider, MemoryStore, RecordingAudit, Scratch};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use porter_sync::{BaseVersion, Conflict, RemoteId, RemoteSide, RemoteVersion, StoredConflict};
use serde_json::Value;
use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use syncd::service::{Access, DatasetName, Event, Hub, Nudge, serve};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const ACCOUNT: &str = "graph-acct";
const APP: &str = "org.example.Rig";
const DATASET: &str = "a1/notes";

const PROVIDER: &str = r#"
id = "fake-graph"
label = "Fake Microsoft"
mark = "generic"

[auth]
kind = "oauth_pkce"
issuer = "microsoft"

[discovery]
kind = "autoconfig"

[[capability]]
family = "graph"
kind = "storage"
v = { access = "read_write", delta = "poll", quota = "reported", scope = "app_folder", hashes = "quick_xor", ranges = "present", chunked_upload = "present" }
"#;

struct Rig {
    bus: PrivateBus,
    hub: Hub,
    settled: Arc<Mutex<Vec<(i64, String)>>>,
    _keep: Vec<zbus::Connection>,
    /// Last, so it goes after the bus and the connections.
    proc_root: Scratch,
}

impl Rig {
    /// `porter-rig-client --app-id org.example.Rig --proc-root <root> <args>`, on the bus.
    fn client(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_porter-rig-client"));
        command
            .env_clear()
            .env("DBUS_SESSION_BUS_ADDRESS", self.bus.address())
            .args(["--app-id", APP, "--proc-root"])
            .arg(self.proc_root.path())
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        command
    }

    /// Runs the client to its end: its JSON lines and whether it succeeded.
    async fn run(&self, args: &[&str]) -> (Vec<Value>, bool) {
        let output = self.client(args).output().await.expect("the client runs");
        let lines = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).expect("a JSON line"))
            .collect();
        (lines, output.status.success())
    }
}

async fn rig() -> Rig {
    let provider = FakeProvider::from_file(PROVIDER);
    let spec = provider.spec();
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: spec.id.clone(),
        label: AccountLabel("ada@outlook.test".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthPkce,
        capabilities: spec
            .capabilities
            .iter()
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            })
            .collect(),
        restriction: Restriction::none(),
        endpoints: Vec::new(),
    };
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: account.id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new("SECRET-ACCESS-TOKEN"),
                refresh: SecretText::new("SECRET-REFRESH-TOKEN"),
                expires_at: UnixSeconds(1),
                granted: Default::default(),
            },
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let proc_root = Scratch::new("client-proc");
    // The daemons name callers from the fixture the client writes, as a scenario's do.
    let accountd = bus.connect().await;
    let callers = Arc::new(ProcCallers::with_proc_root(
        accountd.clone(),
        CallerTable::default(),
        proc_root.to_path_buf(),
    ));
    let sheets = BusSheets::new(accountd.clone(), Arc::clone(&callers));
    let service = Arc::new(
        AccountService::new(
            vec![provider],
            Registry {
                grants: vec![],
                accounts: vec![account],
                toggles: vec![],
            },
            secrets,
            sheets,
            FixedClock(porter_fake::NOW),
        )
        .with_store(MemoryStore::default())
        .with_audit(RecordingAudit::default()),
    );
    serve_with(&accountd, service, callers, Options::default())
        .await
        .expect("accountd serves");

    let hub = Hub::default();
    let server = bus.connect().await;
    let sync_callers = Arc::new(ProcCallers::with_proc_root(
        server.clone(),
        CallerTable::default(),
        proc_root.to_path_buf(),
    ));
    serve(&server, hub.clone(), sync_callers)
        .await
        .expect("sync1");
    let handle = hub.register(
        DatasetName::parse(DATASET).expect("name"),
        Access {
            owners: BTreeSet::from([AppName::parse(APP).expect("app")]),
        },
    );
    // The dataset's driver, as far as settling goes: it records what the owner asks.
    let settled = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&settled);
    tokio::spawn(async move {
        let mut own = handle;
        loop {
            match own.nudged().await {
                Nudge::Settle(request) => {
                    recorded
                        .lock()
                        .expect("lock")
                        .push((request.number.0, format!("{:?}", request.how.0)));
                    let _ = request.reply.send(Ok(()));
                }
                Nudge::Pause | Nudge::Confirm(_) | Nudge::SyncNow => {}
                Nudge::Dropped => break,
            }
        }
    });
    Rig {
        bus,
        hub,
        settled,
        _keep: vec![accountd, server],
        proc_root,
    }
}

fn result_of(lines: &[Value]) -> &str {
    lines
        .last()
        .and_then(|l| l["result"].as_str())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_lists_identities_and_continues_as_one_as_the_app_the_fixture_names() {
    let rig = rig().await;

    let (lines, ok) = rig.run(&["identities", "list"]).await;
    assert!(ok, "{lines:?}");
    assert_eq!(result_of(&lines), "ok");
    assert_eq!(lines[0]["identities"][0]["id"], ACCOUNT);

    // Saying it signed in with the identity is the app's, and holds no token.
    let (lines, ok) = rig.run(&["identities", "register-use", ACCOUNT]).await;
    assert!(ok, "{lines:?}");
    assert_eq!(result_of(&lines), "registered");

    // No sheet host is on this bus, so the picker cannot be shown: the command reached accountd
    // and says what it answered, exit 1. An identity id that is none never reaches the bus.
    let (lines, ok) = rig.run(&["identities", "offer"]).await;
    assert!(!ok, "{lines:?}");
    assert_eq!(result_of(&lines), "error", "{lines:?}");
    let (lines, ok) = rig.run(&["identities", "register-use", "A B"]).await;
    assert!(!ok);
    assert_eq!(result_of(&lines), "error", "{lines:?}");

    // The fixture is removed when the client ends.
    let left: Vec<_> = std::fs::read_dir(rig.proc_root.path())
        .expect("proc")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_lists_datasets_watches_conflicts_and_settles_one() {
    let rig = rig().await;
    let (lines, ok) = rig.run(&["datasets"]).await;
    assert!(ok, "{lines:?}");
    assert_eq!(lines[0]["datasets"][0], DATASET);

    // watch-conflicts joins syncd's roster, says so, and prints the next conflict.
    let mut watching = rig
        .client(&["watch-conflicts", "--count", "1", "--timeout-s", "60"])
        .spawn()
        .expect("spawns");
    let stdout = watching.stdout.take().expect("piped");
    let mut lines = BufReader::new(stdout).lines();
    let first = tokio::time::timeout(porter_fake::GENEROUS, lines.next_line())
        .await
        .expect("a line in time")
        .expect("read")
        .expect("a line");
    let first: Value = serde_json::from_str(&first).expect("JSON");
    assert_eq!(first["event"], "watching");
    assert_eq!(first["datasets"][0], DATASET);

    let name = DatasetName::parse(DATASET).expect("name");
    let stored = StoredConflict {
        number: Some(7),
        conflict: Conflict {
            item: RemoteId("r1".into()),
            base: BaseVersion::Absent,
            remote: RemoteSide::Exists(RemoteId("r1".into()), RemoteVersion("v2".into())),
        },
        local: porter_sync::LocalId("a.txt".into()),
        local_hash: None,
        at: UnixSeconds(5),
    };
    // Any handle of the hub may tell: the dataset's own is held by the driver task, so a second
    // registration under another name would not be seen; use the event the hub relays.
    let teller = rig.hub.register(
        DatasetName::parse("a1/other").expect("name"),
        Access {
            owners: BTreeSet::from([AppName::parse(APP).expect("app")]),
        },
    );
    teller.tell(Event::Conflict {
        dataset: name,
        conflict: Box::new(stored),
    });
    let second = tokio::time::timeout(porter_fake::GENEROUS, lines.next_line())
        .await
        .expect("a line in time")
        .expect("read")
        .expect("a line");
    let second: Value = serde_json::from_str(&second).expect("JSON");
    assert_eq!(second["event"], "conflict");
    assert_eq!(second["dataset"], DATASET);
    assert_eq!(second["conflict"]["number"], 7);
    let status = watching.wait().await.expect("ends");
    assert!(status.success(), "--count 1 ends it after one");

    let (lines, ok) = rig
        .run(&["sync-resolve", DATASET, "7", "keep_remote"])
        .await;
    assert!(ok, "{lines:?}");
    assert_eq!(result_of(&lines), "resolved");
    assert_eq!(
        rig.settled.lock().expect("lock").as_slice(),
        [(7, "KeepRemote".to_owned())]
    );

    let (lines, ok) = rig.run(&["sync-resolve", DATASET, "7", "keep_both"]).await;
    assert!(!ok);
    assert_eq!(result_of(&lines), "error");
    assert!(
        lines[0]["name"]
            .as_str()
            .is_some_and(|n| n.contains("InvalidArgs")),
        "{lines:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn add_account_and_reauthenticate_drive_accountds_sheets_and_say_how_they_ended() {
    let rig = rig().await;

    // No sheet host is on this bus, so accountd cannot show the sheet: the command reached it
    // (the Request object and its Response) and says what the Response was, exit 1.
    for args in [&["add-account"][..], &["add-account", "fake-graph"][..]] {
        let (lines, ok) = rig.run(args).await;
        assert!(!ok, "{args:?}: {lines:?}");
        assert_eq!(result_of(&lines), "refused", "{args:?}: {lines:?}");
        assert_eq!(
            lines.last().expect("a line")["refusal"],
            "Unavailable",
            "{args:?}"
        );
    }

    // An app holds no account, so it cannot ask for one to be signed in again: to it the account
    // is not there (only the shell and Settings sign an account in again).
    let (lines, ok) = rig.run(&["reauthenticate", ACCOUNT]).await;
    assert!(!ok, "{lines:?}");
    assert_eq!(result_of(&lines), "error", "{lines:?}");
    assert!(
        lines[0]["error"]
            .as_str()
            .is_some_and(|e| e.contains("UnknownObject")),
        "{lines:?}"
    );

    // A word that is no provider or account id never reaches the bus.
    for args in [
        &["add-account", "Not A Provider"][..],
        &["reauthenticate", "A B"][..],
    ] {
        let (lines, ok) = rig.run(args).await;
        assert!(!ok, "{args:?}");
        assert_eq!(result_of(&lines), "error", "{args:?}: {lines:?}");
    }
}
