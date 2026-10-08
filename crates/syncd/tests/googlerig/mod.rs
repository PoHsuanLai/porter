//! The rig of syncd's Google bus tests: the real accountd on a private bus with one Google
//! account whose Drive, Photos upload and Picker endpoints are the fake Google's, the grants
//! syncd would hold, and a `StorageSupervisor` over them. The relay, not syncd, authenticates:
//! the bearer is the fake provider's, and the credential accountd holds appears nowhere.
//!
//! Included by `#[path]` from the test files, so each is its own crate and nothing of the
//! other lanes' `common/mod.rs` changes.
#![allow(dead_code)]

use crate::common::bus::PrivateBus;
use crate::common::{Known, client};
use accountd::{BusSheets, Options, TableCallers, serve_with};
use porter_client::{Accounts, DbusTransport};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, Candidate,
    CapabilityKind, Claim, Credential, DataClass, EndpointUrl, Family, GrantId, Isolation,
    LoginName, Offer, Provenance, Restriction, SecretKey, SecretPurpose, SecretText,
    ServiceEndpoint, SpaceScope, Subject, Tls, UnixSeconds,
};
use porter_dbus::{Caller, CallerRole};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::google::MediaOrigin;
use porter_fake_servers::{FakeGoogle, GoogleHandle, Running};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use syncd::datasets::photos::PhotosSwitch;
use syncd::datasets::pim::{AccountdUnavailable, Wiring};
use syncd::datasets::storage::{
    ClientStorageGrants, StorageConfig, StorageGrants, StorageKind, StorageSupervisor,
};
use syncd::paths::{AccountDir, Paths};
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access, Hub, serve};
use tokio::sync::watch;

pub const ACCOUNT: &str = "google-acct";
pub const SEGMENT: &str = "google_acct";
pub const SYNCD: &str = "org.quire.Sync";
pub const SECRET_ACCESS: &str = "SECRET-ACCESS-TOKEN";
pub const SECRET_REFRESH: &str = "SECRET-REFRESH-TOKEN";
/// What the fake provider mints for each audience: `fake:<account>:<family slug>`.
pub const DRIVE_BEARER: &str = "fake:google-acct:google_drive";
pub const UPLOAD_BEARER: &str = "fake:google-acct:google_photos_upload";
pub const PICKER_BEARER: &str = "fake:google-acct:google_photos_picker";
pub const FILES_DATASET: &str = "google_acct/storage_app_folder";
pub const UPLOAD_DATASET: &str = "google_acct/google_photos_upload";

const PROVIDER: &str = r#"
id = "fake-google"
label = "Fake Google"
mark = "generic"

[auth]
kind = "oauth_pkce"
issuer = "google"

[discovery]
kind = "fixed"

[[capability]]
family = "google_drive"
endpoint = "https://www.googleapis.com/drive/v3"
kind = "storage"
v = { access = "read_write", delta = "poll", quota = "reported", scope = "app_folder", hashes = "md5", ranges = "present", chunked_upload = "present" }

[[capability]]
family = "google_photos_upload"
endpoint = "https://photoslibrary.googleapis.com/v1"
kind = "photos"
v = { library_read = "picker_only", upload = "present", albums = "app_created", video = "present", delta = "none" }

[[capability]]
family = "google_photos_picker"
endpoint = "https://photospicker.googleapis.com/v1"
kind = "photos"
v = { library_read = "picker_only", upload = "present", albums = "app_created", video = "present", delta = "none" }
"#;

pub fn app(name: &str) -> AppId {
    AppId {
        name: AppName::parse(name).expect("app"),
        isolation: Isolation::Flatpak,
    }
}

pub fn quick() -> Settings {
    Settings {
        poll_base: 1,
        poll_max: 1,
        push_window: 0,
        batch_window: 0,
        metered: MeteredPolicy::Pause,
    }
}

/// The real grants, until the test hides them (a revoked grant, a removed account), by kind.
#[derive(Debug)]
pub struct Gate {
    pub real: ClientStorageGrants<DbusTransport>,
    pub files: Arc<AtomicBool>,
    pub photos: Arc<AtomicBool>,
}

impl StorageGrants for Gate {
    async fn granted(&self, kind: StorageKind) -> Result<Vec<Candidate>, AccountdUnavailable> {
        let shown = match kind {
            StorageKind::AppFolder => &self.files,
            StorageKind::Photos | StorageKind::GooglePhotos => &self.photos,
        };
        match shown.load(Ordering::SeqCst) {
            true => self.real.granted(kind).await,
            false => Ok(Vec::new()),
        }
    }
}

/// Where the picked photos' bytes are served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    /// On the Picker API's own origin.
    Same,
    /// On a second origin that wants the bearer, which the provider file lists in `auth_origins`.
    Listed,
    /// On a second origin that wants the bearer and that the provider file does not list.
    Unlisted,
}

pub struct Rig {
    pub media: Option<Running<MediaOrigin>>,
    pub bus: PrivateBus,
    pub known: Known,
    pub hub: Hub,
    pub google: Running<GoogleHandle>,
    pub paths: Paths,
    pub account: AccountDir,
    pub files_shown: Arc<AtomicBool>,
    pub photos_shown: Arc<AtomicBool>,
    pub supervisor: StorageSupervisor<DbusTransport, Gate>,
    pub accounts: Arc<Accounts<DbusTransport>>,
    pub network: watch::Receiver<Network>,
    pub photos: PhotosSwitch,
    pub _keep: Vec<zbus::Connection>,
    pub _network: watch::Sender<Network>,
}

fn grant(id: &str, kind: CapabilityKind, class: DataClass) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("grant"),
        key: GrantKey {
            app: app(SYNCD),
            account: AccountId::parse(ACCOUNT).expect("id"),
            kind,
            class,
            usage: Usage::Background,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

fn endpoint(family: Family, url: String) -> ServiceEndpoint {
    ServiceEndpoint {
        family,
        url: EndpointUrl::parse(&url).expect("url"),
        tls: Tls::Plain,
        login: LoginName("ada@gmail.test".into()),
    }
}

impl Rig {
    /// A supervisor over this rig's accountd and gates (a daemon restarting).
    pub fn new_supervisor(&self) -> StorageSupervisor<DbusTransport, Gate> {
        let wiring = Wiring {
            accounts: Arc::clone(&self.accounts),
            hub: self.hub.clone(),
            paths: self.paths.clone(),
            settings: quick(),
            network: self.network.clone(),
            owners: Access::default(),
        };
        StorageSupervisor::new(
            wiring,
            Gate {
                real: ClientStorageGrants::new(Arc::clone(&self.accounts)),
                files: Arc::clone(&self.files_shown),
                photos: Arc::clone(&self.photos_shown),
            },
            StorageConfig {
                photos: self.photos,
                ..StorageConfig::default()
            },
        )
    }
}

/// The rig, with Google Photos behind `photos`.
pub async fn rig(photos: PhotosSwitch) -> Rig {
    rig_with(photos, Media::Same).await
}

/// [`rig`], with the picked photos' bytes where `media` says.
pub async fn rig_with(photos: PhotosSwitch, media: Media) -> Rig {
    build(photos, media, false).await
}

/// [`rig`] for an account where the person granted the picker scope alone: no upload endpoint,
/// no upload capability (what the Google family leaves of Photos then).
pub async fn rig_picker_only(photos: PhotosSwitch) -> Rig {
    build(photos, Media::Same, true).await
}

async fn build(photos: PhotosSwitch, media: Media, picker_only: bool) -> Rig {
    let google = FakeGoogle::start_fixed(DRIVE_BEARER).await.expect("google");
    google.accept_bearer(UPLOAD_BEARER);
    google.accept_bearer(PICKER_BEARER);
    google.drive_set_limit(1_000_000_000);
    let base = google.base_url().to_owned();
    let origin = match media {
        Media::Same => None,
        Media::Listed | Media::Unlisted => Some(google.serve_media().await.expect("media origin")),
    };
    let listed = match (media, &origin) {
        (Media::Listed, Some(origin)) => Some(origin.base_url().replacen("http://", "", 1)),
        (Media::Unlisted, _) => Some("photos.elsewhere.invalid".to_owned()),
        _ => None,
    };
    let text = match listed {
        Some(host) => PROVIDER.replacen(
            "endpoint = \"https://photospicker.googleapis.com/v1\"\n",
            &format!("endpoint = \"https://photospicker.googleapis.com/v1\"\nauth_origins = [\"{host}\"]\n"),
            1,
        ),
        None => PROVIDER.to_owned(),
    };
    let provider = FakeProvider::from_file(&text);
    let spec = provider.spec();
    let mut account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: spec.id.clone(),
        label: AccountLabel("ada@gmail.test".into()),
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
        endpoints: vec![
            endpoint(Family::GoogleDrive, format!("{base}/drive/v3")),
            endpoint(Family::GooglePhotosUpload, format!("{base}/v1")),
            // Google's two Photos APIs are on two hosts; here they share the fake's origin, so
            // the path tells accountd's endpoint match (exact URL) which is which. The clients
            // use only the origin.
            endpoint(Family::GooglePhotosPicker, format!("{base}/v1/picker")),
        ],
    };
    if picker_only {
        account
            .endpoints
            .retain(|e| e.family != Family::GooglePhotosUpload);
        for claim in &mut account.capabilities {
            if let Offer::Present(porter_core::Capability::Photos(cap)) = &mut claim.offer {
                cap.upload = porter_core::capability::Offered::Absent;
            }
        }
    }
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: account.id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new(SECRET_ACCESS),
                refresh: SecretText::new(SECRET_REFRESH),
                expires_at: UnixSeconds(1),
            },
        )
        .await
        .expect("secret");

    let bus = PrivateBus::start();
    let table = Arc::new(TableCallers::new());
    let accountd = bus.connect().await;
    let sheets = BusSheets::new(accountd.clone(), Arc::clone(&table));
    let service = Arc::new(
        AccountService::new(
            vec![provider],
            Registry {
                grants: vec![
                    grant("files-grant", CapabilityKind::Storage, DataClass::Files),
                    grant("photos-grant", CapabilityKind::Photos, DataClass::Photos),
                ],
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

    let home = bus.scratch().join("syncd-home");
    let paths = Paths::resolve(|name| match name {
        "HOME" => Some(home.display().to_string()),
        _ => None,
    })
    .expect("paths");
    let (network_keeps, network) = watch::channel(Network::Unmetered);
    let mut rig = Rig {
        media: origin,
        bus,
        known,
        hub,
        google,
        paths,
        account: AccountDir::parse(SEGMENT).expect("segment"),
        files_shown: Arc::new(AtomicBool::new(true)),
        photos_shown: Arc::new(AtomicBool::new(true)),
        // Replaced just below, once the rig it is made from exists.
        supervisor: placeholder(&accounts, &network),
        accounts,
        network,
        photos,
        _keep: vec![accountd, syncd_side, server],
        _network: network_keeps,
    };
    rig.supervisor = rig.new_supervisor();
    rig
}

/// A supervisor that is never ticked, to fill the field until the real one can be made.
fn placeholder(
    accounts: &Arc<Accounts<DbusTransport>>,
    network: &watch::Receiver<Network>,
) -> StorageSupervisor<DbusTransport, Gate> {
    let hub = Hub::default();
    let paths = Paths::resolve(|_| Some("/nonexistent".into())).expect("paths");
    StorageSupervisor::new(
        Wiring {
            accounts: Arc::clone(accounts),
            hub,
            paths,
            settings: quick(),
            network: network.clone(),
            owners: Access::default(),
        },
        Gate {
            real: ClientStorageGrants::new(Arc::clone(accounts)),
            files: Arc::new(AtomicBool::new(false)),
            photos: Arc::new(AtomicBool::new(false)),
        },
        StorageConfig::default(),
    )
}

/// A client of the bus named `name`, as the app it is.
pub async fn client_of(rig: &Rig, name: &str) -> zbus::Connection {
    client(&rig.bus, &rig.known, name, CallerRole::App).await
}

pub fn local(rig: &Rig, rel: &str) -> std::path::PathBuf {
    rig.paths.storage_dir(&rig.account).join(rel)
}
