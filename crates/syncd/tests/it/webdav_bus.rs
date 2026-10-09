//! syncd syncing a folder against a fake Nextcloud through accountd's `Tokens.OpenAuthenticated`,
//! on one private bus: accountd (the real service over in-memory secrets) holds the app password
//! and runs the relay; syncd's replica gets only a descriptor and so no credential. `Sync1.Status`
//! shows the quota the WebDAV server reports, and an expired sync token uploads nothing again.

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::{Known, client, eventually};
use porter_client::{Accounts, DbusTransport};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    AccountId, AppId, AppName, AuthKind, CapabilityKind, Credential, DataClass, EndpointUrl,
    Family, GrantId, Isolation, LoginName, SecretKey, SecretPurpose, SecretText, SpaceScope, Tls,
    UnixSeconds,
};
use porter_dbus::{Caller, CallerRole, STATUS_KEY_QUOTA, SyncProxy};
use porter_fake::{FixedClock, MemoryStore, RecordingAudit, cloud_provider, storage_account};
use porter_fake_servers::{FakeNextcloud, NextcloudHandle, Running};
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use porter_sync::ConflictRule;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use storage_webdav::Clock;
use syncd::clock::SystemClock;
use syncd::dataset::MemoryDataset;
use syncd::driver::Driver;
use syncd::engine::Engine;
use syncd::journal::Journal;
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access as Visible, DatasetName, Hub, serve};
use syncd::webdav::webdav_replica;
use tokio::sync::{Notify, watch};
use zbus::zvariant::OwnedValue;

const PASSWORD: &str = "S3CRET-NEXTCLOUD-APP-PASSWORD";
const GRANT: &str = "sync-grant";
const SYNCD: &str = "org.quire.Sync";
const PHOTOS: &str = "org.quire.Photos";

fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app"),
        isolation: Isolation::Flatpak,
    }
}

fn quick() -> Settings {
    Settings {
        poll_base: 1,
        poll_max: 1,
        push_window: 0,
        batch_window: 0,
        metered: MeteredPolicy::Pause,
    }
}

async fn status_quota(sync: &SyncProxy<'_>, dataset: &str) -> Option<(u64, Option<u64>)> {
    let status = sync.status(dataset).await.ok()?;
    let quota: std::collections::HashMap<String, OwnedValue> = status
        .get(STATUS_KEY_QUOTA)?
        .try_clone()
        .ok()?
        .try_into()
        .ok()?;
    let used = u64::try_from(&quota["used"]).ok()?;
    Some((used, quota.get("total").and_then(|t| u64::try_from(t).ok())))
}

struct Rig {
    bus: PrivateBus,
    hub: Hub,
    known: Known,
    nextcloud: Running<NextcloudHandle>,
    accounts: Arc<Accounts<DbusTransport>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    _keep: Vec<zbus::Connection>,
}

async fn rig() -> Rig {
    let nextcloud = FakeNextcloud::start("alice").await.expect("nextcloud");
    nextcloud.seed_app_password(PASSWORD);
    nextcloud.mkdir("Photos");
    nextcloud.set_limit(1_000_000);
    let url = format!("{}/remote.php/dav/files/alice/", nextcloud.base_url());

    let mut account = storage_account();
    account.auth = AuthKind::AppPassword;
    account.endpoints.retain(|e| e.family == Family::WebDav);
    account.endpoints[0].url = EndpointUrl::parse(&url).expect("url");
    account.endpoints[0].tls = Tls::Plain;
    account.endpoints[0].login = LoginName("alice".into());
    let endpoint = account.endpoints[0].url.clone();
    let grant = Grant {
        id: GrantId::parse(GRANT).expect("grant"),
        key: GrantKey {
            app: app(SYNCD),
            account: account.id.clone(),
            kind: CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    };
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: AccountId::parse(account.id.as_str()).expect("id"),
                purpose: SecretPurpose::Password,
            },
            &Credential::Password(SecretText::new(PASSWORD)),
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let table = Arc::new(TableCallers::new());
    let accountd = bus.connect().await;
    let sheets = BusSheets::new(accountd.clone(), Arc::clone(&table));
    let service = Arc::new(
        AccountService::new(
            vec![cloud_provider()],
            Registry {
                grants: vec![grant],
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
    serve_with(&accountd, service, Arc::clone(&table), Options::default())
        .await
        .expect("accountd serves");
    let syncd_side = bus.connect().await;
    table.introduce_as(
        syncd_side.unique_name().expect("name").as_str(),
        Caller {
            app: app(SYNCD),
            role: CallerRole::App,
        },
    );
    let accounts = Arc::new(Accounts::over(DbusTransport::over(syncd_side.clone())));

    let known = Known::default();
    let hub = Hub::default();
    let server = bus.connect().await;
    serve(&server, hub.clone(), known.clone())
        .await
        .expect("sync1");
    Rig {
        bus,
        hub,
        known,
        nextcloud,
        accounts,
        grant: GrantId::parse(GRANT).expect("grant"),
        endpoint,
        _keep: vec![accountd, syncd_side, server],
    }
}

fn puts(nextcloud: &NextcloudHandle) -> usize {
    nextcloud
        .hits()
        .iter()
        .filter(|h| h.method == "PUT")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_syncs_against_nextcloud_through_the_relay_and_status_shows_the_quota() {
    let rig = rig().await;
    let dataset = Arc::new(MemoryDataset::new(
        "photos_originals",
        ConflictRule::Impossible,
    ));
    let replica = webdav_replica(
        Arc::clone(&rig.accounts),
        rig.grant.clone(),
        rig.endpoint.clone(),
        "Photos",
        Clock::system(),
    )
    .expect("replica");
    let journal = rig.bus.scratch().join("journal");
    let engine = Engine::new(
        replica,
        Arc::clone(&dataset),
        Journal::open(&journal.join("photos_originals.sqlite")).expect("journal"),
        SystemClock,
    );
    let name = DatasetName::parse("a1/photos_originals").expect("name");
    let handle = rig.hub.register(
        name,
        Visible {
            owners: BTreeSet::from([AppName::parse(PHOTOS).expect("app")]),
        },
    );
    let (_net, network) = watch::channel(Network::Unmetered);
    let driver = Driver::new(engine, handle, quick(), network, Arc::new(Notify::new()), 3);
    let running = tokio::spawn(driver.run());

    let photos = client(&rig.bus, &rig.known, PHOTOS, CallerRole::App).await;
    let sync = SyncProxy::new(&photos).await.expect("proxy");

    // Up: a local file reaches the server, and Status carries the quota the server reports.
    dataset.put("2026/IMG_1.HEIC", b"abcdef");
    eventually("the file is on the server", || puts(&rig.nextcloud) == 1).await;
    let mut quota = None;
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        quota = status_quota(&sync, "a1/photos_originals").await;
        if quota == Some((6, Some(1_000_000))) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(quota, Some((6, Some(1_000_000))));

    // Down: a file put on the server arrives locally.
    rig.nextcloud.put_file("Photos/from-phone.jpg", b"xyz");
    eventually("the remote file arrives", || {
        dataset.get("from-phone.jpg").as_deref() == Some(b"xyz".as_slice())
    })
    .await;

    // The relay, not syncd, authenticated: every request the server saw carries the app
    // password's Basic header, which syncd never held.
    use base64::Engine as _;
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("alice:{PASSWORD}"))
    );
    let dav: Vec<_> = rig
        .nextcloud
        .hits()
        .into_iter()
        .filter(|h| h.target.starts_with("/remote.php/dav"))
        .collect();
    assert!(!dav.is_empty());
    assert!(
        dav.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );

    // An expired sync token: the next cycle lists again and uploads nothing.
    let before = puts(&rig.nextcloud);
    rig.nextcloud.expire_sync_tokens();
    rig.nextcloud.put_file("Photos/poke.txt", b"p");
    eventually("the poke arrives", || dataset.get("poke.txt").is_some()).await;
    assert_eq!(
        puts(&rig.nextcloud),
        before,
        "no re-upload after the expiry"
    );
    assert_eq!(dataset.snapshot().len(), 3);

    rig.hub
        .forget_account(&syncd::paths::AccountDir::parse("a1").expect("account"));
    eventually("the driver stops", || running.is_finished()).await;
}
