//! syncd syncing a folder against a fake Nextcloud through its own account service's authenticated
//! relay, on one private bus: the service (hosted in process over in-memory secrets) holds the app
//! password and runs the relay; syncd's replica gets only a stream and so no credential.
//! `Sync1.Status` shows the quota the WebDAV server reports, and an expired sync token uploads
//! nothing again.

use crate::common;
use porter_ids::clock::SystemClock;

use common::bus::PrivateBus;
use common::hosted::{Link, host};
use common::{Known, client};
use porter_callers::CallerRole;
use porter_capability::CapabilityKind;
use porter_capability::StorageScope;
use porter_client::Grants;
use porter_core::{AccountId, GrantId};
use porter_core::{
    AppName, AuthKind, Credential, DataClass, EndpointUrl, Family, LoginName, SecretKey,
    SecretPurpose, SecretText, SpaceScope, Tls,
};
use porter_core::{
    GrantKey,
    consent::{Decision, Grant, GrantScope, Usage},
};
use porter_dbus::sync::STATUS_KEY_QUOTA;
use porter_dbus::sync::SyncProxy;
use porter_fake::{cloud_provider, storage_account};
use porter_fake_servers::{FakeNextcloud, NextcloudHandle, Running};
use porter_ids::UnixSeconds;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service_core::registry::Registry;
use porter_sync::Clock;
use porter_sync::ConflictRule;
use std::collections::BTreeSet;
use std::sync::Arc;
use syncd::clock::Wall;
use syncd::dataset::MemoryDataset;
use syncd::driver::Driver;
use syncd::engine::Engine;
use syncd::journal::Journal;
use syncd::keep::sync_app;
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access as Visible, DatasetName, Hub, serve};
use syncd::webdav::webdav_replica;
use tokio::sync::{Notify, watch};
use zbus::zvariant::OwnedValue;

const PASSWORD: &str = "S3CRET-NEXTCLOUD-APP-PASSWORD";
const GRANT: &str = "sync-grant";
const PHOTOS: &str = "org.quire.Photos";

fn quick() -> Settings {
    Settings::quick()
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
    accounts: Arc<Grants<Link>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    _keep: Vec<zbus::Connection>,
}

impl Rig {
    /// What the daemon has done so far, as [`common::pulse`] counts it.
    fn pulse(&self) -> u64 {
        common::pulse(&self.hub, self.nextcloud.hits().len())
    }
}

/// Polls every 20 ms; the limit is idle time (`common::eventually_alive`): the wait ends only
/// when the rig has done nothing for [`test_kit::GENEROUS`].
async fn eventually(rig: &Rig, what: &str, check: impl FnMut() -> bool) {
    common::eventually_alive(what, || rig.pulse(), check).await;
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
            app: sync_app(),
            account: account.id.clone(),
            kind: CapabilityKind::Storage,
            class: DataClass::Photos,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
            reach: StorageScope::AppFolder,
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
    let hosted = host(
        cloud_provider(),
        Registry {
            grants: vec![grant],
            accounts: vec![account],
            toggles: vec![],
        },
        secrets,
    );
    let accounts = hosted.accounts;

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
        _keep: vec![server],
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
        Clock::new(SystemClock),
        "t1",
    )
    .expect("replica");
    let journal = rig.bus.scratch().join("journal");
    let engine = Engine::new(
        replica,
        Arc::clone(&dataset),
        Journal::open(&journal.join("photos_originals.sqlite")).expect("journal"),
        Wall::scaled(quick().time_scale),
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
    eventually(&rig, "the file is on the server", || {
        puts(&rig.nextcloud) == 1
    })
    .await;
    common::until_alive(
        "Status carries the quota",
        || rig.pulse(),
        || async { status_quota(&sync, "a1/photos_originals").await == Some((6, Some(1_000_000))) },
    )
    .await;

    // Down: a file put on the server arrives locally.
    rig.nextcloud.put_file("Photos/from-phone.jpg", b"xyz");
    eventually(&rig, "the remote file arrives", || {
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
    eventually(&rig, "the poke arrives", || {
        dataset.get("poke.txt").is_some()
    })
    .await;
    assert_eq!(
        puts(&rig.nextcloud),
        before,
        "no re-upload after the expiry"
    );
    assert_eq!(dataset.snapshot().len(), 3);

    rig.hub
        .forget_account(&syncd::paths::AccountDir::parse("a1").expect("account"));
    eventually(&rig, "the driver stops", || running.is_finished()).await;
}
