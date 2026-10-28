//! syncd syncing a folder of OneDrive's app folder against a fake Graph drive through its own
//! account service's authenticated relay, on one private bus: the service (hosted in process over
//! in-memory secrets, a fake provider that mints a token for the endpoint's audience) holds the
//! OAuth credential and runs the relay, which adds the bearer; syncd's replica gets only a stream
//! and so no token. The endpoint is a real `graph` one. The fake drive hands out its upload
//! sessions and redirected downloads on a SECOND loopback origin, which the provider file declares
//! in `linked_origins`; the replica reaches it through the linked relay, which adds no
//! credential. `Sync1.Status` shows the quota the drive reports, a large file goes up in an upload
//! session, and an expired delta token uploads nothing again.

use crate::common;
use porter_ids::clock::SystemClock;

use common::bus::PrivateBus;
use common::hosted::{Link, host};
use common::{Known, client};
use porter_callers::CallerRole;
use porter_capability::StorageScope;
use porter_capability::{CapabilityKind, Claim, Offer, Provenance, Subject};
use porter_client::Grants;
use porter_core::{
    Account, AccountLabel, AccountState, AppName, AuthKind, Credential, DataClass, EndpointUrl,
    Family, LoginName, Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint,
    SpaceScope, Tls,
};
use porter_core::{AccountId, GrantId};
use porter_core::{
    GrantKey,
    consent::{Decision, Grant, GrantScope, Usage},
};
use porter_dbus::sync::STATUS_KEY_QUOTA;
use porter_dbus::sync::SyncProxy;
use porter_fake::FakeProvider;
use porter_fake_servers::graph::Knobs;
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_http_relay::RelayStream;
use porter_ids::UnixSeconds;
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service_core::registry::Registry;
use porter_sync::Clock;
use porter_sync::ConflictRule;
use std::collections::BTreeSet;
use std::sync::Arc;
use storage_graph::{CHUNK_UNIT, Uploads};
use syncd::clock::Wall;
use syncd::dataset::MemoryDataset;
use syncd::driver::Driver;
use syncd::engine::Engine;
use syncd::graph::graph_replica;
use syncd::journal::Journal;
use syncd::keep::sync_app;
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access as Visible, DatasetName, Hub, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};
use zbus::zvariant::OwnedValue;

const GRANT: &str = "sync-grant";
const ACCOUNT: &str = "graph-acct";
const PHOTOS: &str = "org.quire.Photos";
/// What the fake provider mints for the audience of this account's endpoint (its family's slug).
const BEARER: &str = "fake:graph-acct:graph";

/// A Microsoft-shaped provider: OAuth, Storage over a `graph` endpoint, with the origin of the
/// fake's links (its second loopback port) declared as one pre-authenticated links may point at.
fn provider_file(linked: &str) -> String {
    format!(
        r#"
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
linked_origins = ["{linked}"]
v = {{ access = "read_write", delta = "poll", quota = "reported", scope = "app_folder", hashes = "quick_xor", ranges = "present", chunked_upload = "present" }}
"#
    )
}

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
    graph: Running<GraphHandle>,
    accounts: Arc<Grants<Link>>,
    grant: GrantId,
    endpoint: EndpointUrl,
    link: EndpointUrl,
    _keep: Vec<zbus::Connection>,
}

impl Rig {
    /// What the daemon has done so far, as [`common::pulse`] counts it, with both origins' hits.
    fn pulse(&self) -> u64 {
        common::pulse(
            &self.hub,
            self.graph.hits().len() + self.graph.link_hits().len(),
        )
    }
}

/// Polls every 20 ms; the limit is idle time (`common::eventually_alive`): the wait ends only
/// when the rig has done nothing for [`test_kit::GENEROUS`].
async fn eventually(rig: &Rig, what: &str, check: impl FnMut() -> bool) {
    common::eventually_alive(what, || rig.pulse(), check).await;
}

fn account(provider: &FakeProvider, url: &EndpointUrl) -> Account {
    let spec = provider.spec();
    Account {
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
        endpoints: vec![ServiceEndpoint {
            family: Family::Graph,
            url: url.clone(),
            tls: Tls::Plain,
            login: LoginName("ada@outlook.test".into()),
        }],
    }
}

async fn rig() -> Rig {
    let graph = FakeGraph::start_linked(BEARER).await.expect("graph");
    graph.set_limit(1_000_000);
    graph.set_knobs(Knobs {
        redirect_downloads: true,
        ..Knobs::default()
    });
    let endpoint = EndpointUrl::parse(graph.base_url()).expect("url");
    let link = EndpointUrl::parse(graph.link_url().expect("a linked fake")).expect("url");
    let declared = link.as_str().trim_start_matches("http://").to_owned();
    let provider = FakeProvider::from_file(&provider_file(&declared));
    let account = account(&provider, &endpoint);
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
    let hosted = host(
        provider,
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
        graph,
        accounts,
        grant: GrantId::parse(GRANT).expect("grant"),
        endpoint,
        link,
        _keep: vec![server],
    }
}

/// Every write the drive or its links' origin saw.
fn uploads(graph: &GraphHandle) -> usize {
    graph
        .hits()
        .iter()
        .chain(graph.link_hits().iter())
        .filter(|h| matches!(h.method.as_str(), "PUT" | "POST"))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_syncs_against_graph_through_the_relay_and_status_shows_the_quota() {
    let rig = rig().await;
    let dataset = Arc::new(MemoryDataset::new(
        "photos_originals",
        ConflictRule::Impossible,
    ));
    // Files past 100 bytes go in an upload session, in chunks of one unit.
    let replica = graph_replica(
        Arc::clone(&rig.accounts),
        rig.grant.clone(),
        rig.endpoint.clone(),
        "Photos",
        Clock::new(SystemClock),
    )
    .expect("replica")
    .with_uploads(Uploads::new(100, CHUNK_UNIT));
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

    // Up: a small file is one PUT, a large one a session; Status carries the quota the drive
    // reports.
    let large: Vec<u8> = (0..CHUNK_UNIT + 1000).map(|i| (i % 253) as u8).collect();
    dataset.put("2026/IMG_1.HEIC", b"abcdef");
    dataset.put("2026/BIG.MOV", &large);
    eventually(&rig, "both files are on the drive", || {
        rig.graph.file("Photos/2026/IMG_1.HEIC").as_deref() == Some(b"abcdef".as_slice())
            && rig.graph.file("Photos/2026/BIG.MOV").as_deref() == Some(large.as_slice())
    })
    .await;
    let used = 6 + large.len() as u64;
    common::until_alive(
        "Status carries the quota",
        || rig.pulse(),
        || async {
            status_quota(&sync, "a1/photos_originals").await == Some((used, Some(1_000_000)))
        },
    )
    .await;
    let link_hits = rig.graph.link_hits();
    assert!(
        link_hits
            .iter()
            .any(|h| h.method == "PUT" && h.target.starts_with("/upload/") && h.status == 201),
        "the large file went through a session on the second origin: {link_hits:?}"
    );
    assert!(
        rig.graph
            .hits()
            .iter()
            .all(|h| !h.target.starts_with("/upload/")),
        "no chunk went to the drive's own origin"
    );

    // Down: a file put on the drive by another device arrives locally.
    rig.graph.put_file("Photos/from-phone.jpg", b"xyz");
    eventually(&rig, "the remote file arrives", || {
        dataset.get("from-phone.jpg").as_deref() == Some(b"xyz".as_slice())
    })
    .await;

    // The relay, not syncd, authenticated: every request the drive saw carries the bearer the
    // fake provider minted, and the credential accountd holds appears nowhere.
    assert!(
        rig.graph
            .link_hits()
            .iter()
            .any(|h| h.method == "GET" && h.target.starts_with("/dl/") && h.status == 200),
        "the remote file came down from the second origin"
    );
    assert!(
        rig.graph
            .link_hits()
            .iter()
            .all(|h| h.authorization.is_none() && h.status != 401),
        "no bearer reached the linked origin: {:?}",
        rig.graph.link_hits()
    );
    let api: Vec<_> = rig.graph.hits();
    assert!(!api.is_empty());
    let want = format!("Bearer {BEARER}");
    assert!(
        api.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );

    // An expired delta token: the next cycle lists again and uploads nothing.
    let before = uploads(&rig.graph);
    rig.graph.expire_delta_tokens();
    rig.graph.put_file("Photos/poke.txt", b"p");
    eventually(&rig, "the poke arrives", || {
        dataset.get("poke.txt").is_some()
    })
    .await;
    assert_eq!(uploads(&rig.graph), before, "no re-upload after the expiry");
    assert_eq!(dataset.snapshot().len(), 4);

    rig.hub
        .forget_account(&syncd::paths::AccountDir::parse("a1").expect("account"));
    eventually(&rig, "the driver stops", || running.is_finished()).await;
}

/// Reads one HTTP response off a relay: its status line and whatever else came with it.
async fn status_of(stream: &mut RelayStream) -> String {
    let mut seen = Vec::new();
    let mut buf = [0u8; 2048];
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await.expect("read");
        assert!(
            n > 0,
            "the relay closed: {}",
            String::from_utf8_lossy(&seen)
        );
        seen.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&seen)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linked_relay_reaches_a_declared_origin_only_adds_no_credential_and_strips_the_apps() {
    let rig = rig().await;

    // An origin the provider file does not declare (the drive's own port is not a linked one,
    // nor is a host that is not on this computer's declared list): refused, no descriptor.
    for origin in [
        rig.endpoint.as_str(),
        "http://127.0.0.1:9",
        "https://files.1drv.com",
    ] {
        let refused = rig
            .accounts
            .open_linked(&rig.grant, &EndpointUrl::parse(origin).expect("url"))
            .await;
        assert!(
            matches!(
                refused,
                Err(porter_client::GrantsError::Refused(
                    porter_wire::Refusal::EndpointNotGranted
                ))
            ),
            "{origin}: {:?}",
            refused.err()
        );
    }
    assert!(rig.graph.link_hits().is_empty() && rig.graph.hits().is_empty());

    // The declared one opens. The app writes a bearer of its own; the relay drops it, and the
    // linked origin (which refuses any Authorization) answers the request itself.
    let stream = rig
        .accounts
        .open_linked(&rig.grant, &rig.link)
        .await
        .expect("a declared origin");
    let mut stream = RelayStream::from_relay(stream).expect("stream");
    let host = rig.link.as_str().trim_start_matches("http://");
    let request =
        format!("GET /dl/missing HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer stolen\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    assert_eq!(status_of(&mut stream).await, "HTTP/1.1 404 Not Found");
    let seen = rig.graph.link_hits();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].authorization, None,
        "no credential, not even the app's"
    );

    // The relay dials that origin only: a request for the drive's host over it is refused.
    let request = format!(
        "GET /v1.0/me/drive HTTP/1.1\r\nHost: {}\r\n\r\n",
        rig.endpoint.as_str().trim_start_matches("http://")
    );
    let _ = stream.write_all(request.as_bytes()).await;
    let mut rest = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&buf[..n]);
    }
    assert!(rig.graph.hits().is_empty(), "the drive saw nothing");
}
