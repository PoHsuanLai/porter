//! The grant that lets syncd keep an account's storage here: made and taken back by Settings,
//! audited, one per account and class, only for an account syncd can mirror.

use porter_core::audit::AuditEvent;
use porter_core::capability::{Albums, LibraryRead, Offered, PhotosCap};
use porter_core::consent::{Decision, GrantScope, Usage};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountId, AppId, AppName, Capability, CapabilityKind, Claim, DataClass, EndpointUrl,
    Family, Isolation, LoginName, Offer, Provenance, ServiceEndpoint, SpaceScope, Subject, Tls,
    Toggle,
};
use porter_fake::{
    FixedClock, MemoryStore, RecordingAudit, ScriptedSheets, cloud_provider, mail_account,
    storage_account,
};
use porter_secrets::MemorySecrets;
use porter_service::{
    AccountService, Registry, SyncClass, photos_key, sync_allowed, sync_app, sync_key, sync_key_of,
    sync_offers,
};

type Svc = AccountService<
    porter_fake::FakeProvider,
    MemorySecrets,
    ScriptedSheets,
    FixedClock,
    MemoryStore,
    RecordingAudit,
>;

/// A storage account at a Graph server, with resumable uploads as `chunked` says.
fn graph_account(chunked: Offered) -> Account {
    let mut account = storage_account();
    account.id = AccountId::parse("fake-graph").expect("id");
    account.endpoints.push(ServiceEndpoint {
        family: Family::Graph,
        url: EndpointUrl::parse("https://graph.invalid").expect("url"),
        tls: Tls::Implicit,
        login: LoginName("ada@cloud.invalid".into()),
    });
    for claim in &mut account.capabilities {
        if let Offer::Present(Capability::Storage(cap)) = &mut claim.offer {
            cap.chunked_upload = chunked;
        }
    }
    account
}

/// A Google account: the Drive app data folder and, as `photos` says, Google Photos (upload and
/// picker), at servers that are not there.
fn google_account(drive: bool, photos: bool) -> Account {
    let mut account = graph_account(Offered::Present);
    account.id = AccountId::parse("fake-google").expect("id");
    account.endpoints.clear();
    let endpoint = |family: Family, url: &str| ServiceEndpoint {
        family,
        url: EndpointUrl::parse(url).expect("url"),
        tls: Tls::Implicit,
        login: LoginName("ada@gmail.invalid".into()),
    };
    if drive {
        account.endpoints.push(endpoint(
            Family::GoogleDrive,
            "https://drive.invalid/drive/v3",
        ));
    } else {
        account.capabilities.clear();
    }
    if photos {
        account.endpoints.push(endpoint(
            Family::GooglePhotosUpload,
            "https://photos.invalid/v1",
        ));
        account.endpoints.push(endpoint(
            Family::GooglePhotosPicker,
            "https://picker.invalid/v1",
        ));
        account.capabilities.push(Claim {
            subject: Subject::Account,
            offer: Offer::Present(Capability::Photos(PhotosCap {
                library_read: LibraryRead::PickerOnly,
                upload: Offered::Present,
                albums: Albums::AppCreated,
                video: Offered::Present,
                delta: porter_core::capability::Delta::None,
            })),
            provenance: Provenance::Declared,
        });
    }
    account
}

fn service(accounts: Vec<Account>) -> (Svc, MemoryStore, RecordingAudit) {
    let store = MemoryStore::default();
    let audit = RecordingAudit::default();
    let service = AccountService::new(
        vec![cloud_provider()],
        Registry {
            accounts,
            ..Registry::default()
        },
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(store.clone())
    .with_audit(audit.clone());
    (service, store, audit)
}

fn id(account: &Account) -> AccountId {
    account.id.clone()
}

#[test]
fn syncd_is_the_native_daemon_org_quire_sync_and_its_key_is_background_storage() {
    let account = graph_account(Offered::Present);
    let key = sync_key(&account.id, SyncClass::Photos);
    assert_eq!(
        sync_app(),
        AppId {
            name: AppName::parse("org.quire.Sync").expect("name"),
            isolation: Isolation::Unsandboxed,
        }
    );
    assert_eq!(key.app, sync_app());
    assert_eq!(
        (key.kind, key.class, key.usage, key.space),
        (
            CapabilityKind::Storage,
            DataClass::Photos,
            Usage::Background,
            SpaceScope::Any
        )
    );
    assert_eq!(
        sync_key(&account.id, SyncClass::Files).class,
        DataClass::Files
    );
}

#[test]
fn a_google_account_where_only_the_picker_was_granted_offers_no_photos_backup() {
    // What the Google family leaves of Photos when the person ticked the picker scope alone: no
    // upload, no albums, and no upload endpoint. The picker stands alone; Drive is unaffected.
    let mut account = google_account(true, true);
    account
        .endpoints
        .retain(|e| e.family != Family::GooglePhotosUpload);
    for claim in &mut account.capabilities {
        if let Offer::Present(Capability::Photos(cap)) = &mut claim.offer {
            cap.upload = Offered::Absent;
            cap.albums = Albums::None;
        }
    }
    assert_eq!(sync_offers(&account), vec![SyncClass::Files]);
    let mut photos_only = account.clone();
    photos_only
        .endpoints
        .retain(|e| e.family == Family::GooglePhotosPicker);
    assert_eq!(sync_offers(&photos_only), vec![]);
    assert!(
        photos_only
            .endpoints
            .iter()
            .any(|e| e.family == Family::GooglePhotosPicker)
    );
}

#[test]
fn an_account_offers_files_over_graph_and_photos_when_uploads_resume() {
    let cases = [
        (
            "graph with resumable uploads",
            graph_account(Offered::Present),
            vec![SyncClass::Files, SyncClass::Photos],
        ),
        (
            "graph without",
            graph_account(Offered::Absent),
            vec![SyncClass::Files],
        ),
        ("webdav storage", storage_account(), vec![]),
        ("mail", mail_account(), vec![]),
        (
            "google with the app data folder and photos",
            google_account(true, true),
            vec![SyncClass::Files, SyncClass::Photos],
        ),
        (
            "google with the app data folder only",
            google_account(true, false),
            vec![SyncClass::Files],
        ),
        (
            "google with photos only (the person unticked Drive)",
            google_account(false, true),
            vec![SyncClass::Photos],
        ),
    ];
    for (name, account, want) in cases {
        assert_eq!(sync_offers(&account), want, "{name}");
    }
}

#[tokio::test]
async fn on_makes_one_always_allow_background_grant_and_off_takes_it_back() {
    let account = graph_account(Offered::Present);
    let (service, store, audit) = service(vec![account.clone()]);
    let key = sync_key(&account.id, SyncClass::Files);

    service
        .set_sync(&id(&account), SyncClass::Files, Toggle::On)
        .await
        .expect("on");
    let registry = service.registry();
    assert_eq!(registry.grants.len(), 1);
    let grant = &registry.grants[0];
    assert_eq!(grant.key, key);
    assert_eq!(
        (grant.decision, grant.scope),
        (Decision::Allow, GrantScope::Always)
    );
    assert!(sync_allowed(
        &registry.grants,
        &account.id,
        SyncClass::Files
    ));
    assert!(!sync_allowed(
        &registry.grants,
        &account.id,
        SyncClass::Photos
    ));
    assert_eq!(store.saves(), 1);

    // On again is the same grant, not a second one, and nothing more is written.
    service
        .set_sync(&id(&account), SyncClass::Files, Toggle::On)
        .await
        .expect("on again");
    assert_eq!(service.registry().grants.len(), 1);
    assert_eq!(store.saves(), 1);

    service
        .set_sync(&id(&account), SyncClass::Files, Toggle::Off)
        .await
        .expect("off");
    assert!(service.registry().grants.is_empty());
    assert_eq!(store.saves(), 2);
    service
        .set_sync(&id(&account), SyncClass::Files, Toggle::Off)
        .await
        .expect("off again");
    assert_eq!(store.saves(), 2);

    let events: Vec<_> = audit.entries().into_iter().map(|e| e.event).collect();
    let granted = grant.id.clone();
    assert_eq!(
        events,
        vec![
            AuditEvent::Granted {
                grant: granted.clone(),
                kind: CapabilityKind::Storage
            },
            AuditEvent::Revoked { grant: granted },
        ]
    );
}

#[tokio::test]
async fn files_and_photos_are_two_grants_and_each_goes_alone() {
    let account = graph_account(Offered::Present);
    let (service, _, _) = service(vec![account.clone()]);
    for class in SyncClass::ALL {
        service
            .set_sync(&id(&account), class, Toggle::On)
            .await
            .expect("on");
    }
    assert_eq!(service.registry().grants.len(), 2);
    service
        .set_sync(&id(&account), SyncClass::Photos, Toggle::Off)
        .await
        .expect("off");
    let grants = service.registry().grants;
    assert!(sync_allowed(&grants, &account.id, SyncClass::Files));
    assert!(!sync_allowed(&grants, &account.id, SyncClass::Photos));
}

#[tokio::test]
async fn an_earlier_refusal_for_syncd_does_not_outlast_the_switch() {
    let account = graph_account(Offered::Present);
    let (first, _, _) = service(vec![account.clone()]);
    first
        .set_sync(&id(&account), SyncClass::Files, Toggle::On)
        .await
        .expect("on");
    // A Deny kept for the same key (as a consent sheet could have stored) is replaced.
    let mut registry = first.registry();
    registry.grants[0].decision = Decision::Deny;
    let service = AccountService::new(
        vec![cloud_provider()],
        registry,
        MemorySecrets::default(),
        ScriptedSheets::default(),
        FixedClock(porter_fake::NOW),
    )
    .with_store(MemoryStore::default());
    service
        .set_sync(&id(&account), SyncClass::Files, Toggle::On)
        .await
        .expect("on over a refusal");
    let grants = service.registry().grants;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].decision, Decision::Allow);
}

#[tokio::test]
async fn an_account_syncd_cannot_mirror_or_one_that_is_not_there_is_refused() {
    let graph = graph_account(Offered::Absent);
    let (service, store, _) = service(vec![graph.clone(), mail_account(), storage_account()]);
    for (name, account, class, want) in [
        (
            "photos without resumable uploads",
            id(&graph),
            SyncClass::Photos,
            Refusal::NoFittingAccount,
        ),
        (
            "mail",
            id(&mail_account()),
            SyncClass::Files,
            Refusal::NoFittingAccount,
        ),
        (
            "webdav storage",
            id(&storage_account()),
            SyncClass::Files,
            Refusal::NoFittingAccount,
        ),
        (
            "nobody",
            AccountId::parse("nobody").expect("id"),
            SyncClass::Files,
            Refusal::UnknownGrant,
        ),
    ] {
        assert_eq!(
            service.set_sync(&account, class, Toggle::On).await,
            Err(want),
            "{name}"
        );
    }
    assert!(service.registry().grants.is_empty());
    assert_eq!(store.saves(), 0);
}

#[tokio::test]
async fn the_photos_of_a_google_account_are_a_photos_kind_grant_and_files_stay_storage() {
    let account = google_account(true, true);
    let (service, _, audit) = service(vec![account.clone()]);
    assert_eq!(
        sync_key_of(&account, SyncClass::Files),
        sync_key(&account.id, SyncClass::Files)
    );
    let photos = sync_key_of(&account, SyncClass::Photos);
    assert_eq!(photos, photos_key(&account.id));
    assert_eq!(
        (photos.kind, photos.class, photos.usage),
        (CapabilityKind::Photos, DataClass::Photos, Usage::Background)
    );

    for class in SyncClass::ALL {
        service
            .set_sync(&id(&account), class, Toggle::On)
            .await
            .expect("on");
    }
    let grants = service.registry().grants;
    let mut kinds: Vec<(DataClass, CapabilityKind)> =
        grants.iter().map(|g| (g.key.class, g.key.kind)).collect();
    kinds.sort_by_key(|(class, _)| format!("{class:?}"));
    assert_eq!(
        kinds,
        [
            (DataClass::Files, CapabilityKind::Storage),
            (DataClass::Photos, CapabilityKind::Photos)
        ]
    );
    assert!(
        SyncClass::ALL
            .iter()
            .all(|class| sync_allowed(&grants, &account.id, *class))
    );
    // On twice is one grant.
    service
        .set_sync(&id(&account), SyncClass::Photos, Toggle::On)
        .await
        .expect("on again");
    assert_eq!(service.registry().grants.len(), 2);

    service
        .set_sync(&id(&account), SyncClass::Photos, Toggle::Off)
        .await
        .expect("off");
    let grants = service.registry().grants;
    assert!(sync_allowed(&grants, &account.id, SyncClass::Files));
    assert!(!sync_allowed(&grants, &account.id, SyncClass::Photos));
    let kinds: Vec<CapabilityKind> = audit
        .entries()
        .into_iter()
        .filter_map(|e| match e.event {
            AuditEvent::Granted { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, [CapabilityKind::Storage, CapabilityKind::Photos]);
}
