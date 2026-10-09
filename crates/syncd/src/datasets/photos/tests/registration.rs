//! The registration seam and the `AccountRemoved` wipe.

use super::super::{DeviceId, ManualMillis, PhotoLibrary, PhotosSwitch, PhotosWiring, start};
use crate::paths::{AccountDir, Paths};
use crate::removal::wipe;
use crate::scheduler::{Network, Settings};
use crate::service::{Access, DatasetName, Hub};
use crate::testing::scratch;
use porter_core::capability::{
    Access as Rights, Delta, HashKind, Offered, QuotaReport, StorageCap, StorageScope,
};
use porter_core::{AppId, AppName, Isolation, UnixSeconds};
use porter_dbus::{Caller, CallerRole};
use porter_sync::MemoryReplica;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn replica() -> MemoryReplica {
    MemoryReplica::new(
        StorageCap {
            access: Rights::ReadWrite,
            delta: Delta::Poll,
            quota: QuotaReport::Unreported,
            scope: StorageScope::AppFolder,
            hashes: HashKind::Sha256,
            ranges: Offered::Present,
            chunked_upload: Offered::Absent,
        },
        100,
        UnixSeconds(1_000),
    )
}

fn wiring(root: &Path) -> PhotosWiring {
    let (_keep, network) = tokio::sync::watch::channel(Network::Unmetered);
    // The sender may go: a closed channel still holds its last value.
    PhotosWiring {
        hub: Hub::default(),
        paths: Paths {
            journals: root.join("state/porter/sync"),
            mirrors: root.join("data/porter/vdir"),
            callers_system: root.join("none"),
            callers_user: root.join("none"),
        },
        settings: Settings::quick(),
        network,
        owners: Access::default(),
    }
}

fn settings_caller() -> Caller {
    Caller {
        app: AppId {
            name: AppName::parse("org.quire.Settings").expect("app"),
            isolation: Isolation::Flatpak,
        },
        role: CallerRole::Settings,
    }
}

fn library(wiring: &PhotosWiring, account: &AccountDir) -> PhotoLibrary {
    PhotoLibrary::open(
        wiring.paths.photos_dir(account),
        DeviceId::parse("a").expect("device"),
        Arc::new(ManualMillis::at(1_000)),
    )
    .expect("library")
}

#[tokio::test]
async fn photos_is_off_by_default_and_registers_both_datasets_when_switched_on() {
    let root = scratch("photos-register");
    let wiring = wiring(&root);
    let account = AccountDir::parse("a1").expect("account");

    let off = start(
        &wiring,
        PhotosSwitch::default(),
        &account,
        library(&wiring, &account),
        replica(),
        replica(),
    )
    .expect("start");
    assert!(off.is_none());
    assert_eq!(
        wiring.hub.names_for(&settings_caller()),
        Vec::<String>::new()
    );
    assert!(!wiring.paths.journals.exists(), "nothing opened while off");

    let run = start(
        &wiring,
        PhotosSwitch::On,
        &account,
        library(&wiring, &account),
        replica(),
        replica(),
    )
    .expect("start")
    .expect("running");
    let mut names = wiring.hub.names_for(&settings_caller());
    names.sort();
    assert_eq!(
        names,
        vec![
            "a1/photos_metadata".to_owned(),
            "a1/photos_originals".to_owned()
        ]
    );

    // It runs: an import is uploaded by the engines to the replica of the datasets.
    let photo = root.join("x.jpg");
    std::fs::write(&photo, b"registered").expect("photo");
    let id = run.library().import(&photo).expect("import").id;
    run.changed();
    let originals = DatasetName::parse("a1/photos_originals").expect("name");
    let mut synced = false;
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        let status = wiring.hub.status_for(&settings_caller(), &originals);
        synced = status.is_some_and(|s| s.anchor_age.is_some());
        if synced {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(synced, "the originals dataset ran a cycle");
    assert!(run.library().original(&id).is_some());
    drop(run);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn account_removed_wipes_the_photos_directory_the_journals_and_the_running_datasets() {
    let root = scratch("photos-wipe");
    let wiring = wiring(&root);
    let (gone, kept) = (
        AccountDir::parse("a1").expect("a1"),
        AccountDir::parse("a2").expect("a2"),
    );
    let mut runs = Vec::new();
    for account in [&gone, &kept] {
        let library = library(&wiring, account);
        let photo = root.join(format!("{account}.jpg"));
        std::fs::write(&photo, account.as_str()).expect("photo");
        library.import(&photo).expect("import");
        runs.push(
            start(
                &wiring,
                PhotosSwitch::On,
                account,
                library,
                replica(),
                replica(),
            )
            .expect("start")
            .expect("running"),
        );
    }
    assert!(wiring.paths.photos_dir(&gone).join("originals").is_dir());
    assert!(wiring.paths.journal(&gone, "photos_originals").is_file());

    let removed = wipe(&wiring.paths, &wiring.hub, &gone).await.expect("wipe");
    assert_eq!(removed, 2, "the journals and the library");
    assert!(!wiring.paths.photos_dir(&gone).exists());
    assert!(!wiring.paths.journals.join("a1").exists());
    assert!(
        wiring.paths.photos_dir(&kept).join("originals").is_dir(),
        "another account is untouched"
    );
    assert!(wiring.paths.journals.join("a2").is_dir());
    assert_eq!(
        wiring.hub.names_for(&settings_caller()),
        vec![
            "a2/photos_metadata".to_owned(),
            "a2/photos_originals".to_owned()
        ]
        .into_iter()
        .collect::<Vec<_>>(),
        "only the other account's datasets still run"
    );
    assert_eq!(
        wipe(&wiring.paths, &wiring.hub, &gone)
            .await
            .expect("again"),
        0
    );
    drop(runs);
    let _ = std::fs::remove_dir_all(root);
}
