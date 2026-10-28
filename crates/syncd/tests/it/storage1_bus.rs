//! `org.quire.Storage1` as syncd serves it, on a private bus over drives held in memory: the
//! frozen interface, who may call, the shapes `Drives` and `Items` answer, the actions and the
//! words they refuse with, Save a Copy, pausing, what was changed here on its way to the drive,
//! and the signals a watcher hears. No folder is shown: nothing here mounts.

use crate::common;

use common::bus::PrivateBus;
use common::{ACCESS_DENIED, Known, client, error_name, eventually};
use porter_callers::CallerRole;
use porter_core::AppName;
use porter_dbus::sync::{
    DRIVE_KEY_LOCAL, DRIVE_KEY_PATH, DRIVE_KEY_PROVIDER, DRIVE_KEY_STATE, DRIVE_KEY_WORDS,
    ITEM_KEY_FRACTION, ITEM_KEY_KEPT, ITEM_KEY_KIND, ITEM_KEY_OTHER, ITEM_KEY_STATE, ITEM_KEY_URL,
    StorageProxy,
};
use porter_dbus::sync::{STORAGE_PATH, SYNC_BUS};
use porter_drive::testing::{MemoryListing, MemorySink, MemorySource, Took};
use porter_drive::{Drive, DriveConfig, NodeId, Reason};
use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use syncd::datasets::drive::{DriveSet, LiveDrive};
use syncd::service::{Access, Hub, serve, serve_storage};
use syncd::words;
use zbus::export::futures_core::Stream;
use zbus::zvariant::OwnedValue;

const FILES: &str = "org.example.Files";
const ROOT: &str = "/home/example/Cloud/OneDrive";

struct Rig {
    bus: PrivateBus,
    known: Known,
    listing: Arc<MemoryListing>,
    source: Arc<MemorySource>,
    drive: Arc<LiveDrive>,
    sink: Arc<MemorySink>,
    _server: zbus::Connection,
}

/// A drive that is read only.
async fn rig() -> Rig {
    rig_over(false).await
}

/// A drive that takes changes, sent through a sink held in memory.
async fn writable() -> Rig {
    rig_over(true).await
}

async fn rig_over(writable: bool) -> Rig {
    let bus = PrivateBus::start();
    let known = Known::default();
    let listing = Arc::new(MemoryListing::default());
    let source = Arc::new(MemorySource::default());
    listing.folder("w", None, "Work");
    listing.file("r", Some("w"), "Report.pdf", 6, "1");
    source.put("r", b"report".to_vec());
    listing.online(
        "doc",
        None,
        "Plan",
        "https://docs.example.test/document/d/doc",
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    );
    listing.folder("k", None, "Kept");
    listing.file("p", Some("k"), "Photo.jpg", 3, "1");
    source.put("p", b"jpg".to_vec());
    let sink = Arc::new(MemorySink::new(listing.clone(), source.clone(), 2));
    let config = DriveConfig {
        chunk: 2,
        quiet: Duration::from_millis(30),
        ..DriveConfig::new(bus.scratch().join("cache/one"))
    };
    let files = match writable {
        true => Drive::open_writable(config, listing.clone(), source.clone(), sink.clone()).await,
        false => Drive::open(config, listing.clone(), source.clone()).await,
    }
    .expect("drive");
    let drive = Arc::new(LiveDrive::new(
        "one".to_owned(),
        "/org/quire/Accounts1/account/one".to_owned(),
        "ada@example.test".to_owned(),
        "OneDrive",
        ROOT.into(),
        files,
    ));
    drive.set_listed(porter_dbus::sync::DriveState::Ready, None);
    let drives = DriveSet::default();
    drives.insert(Arc::clone(&drive));
    let server = bus.connect().await;
    let apps = Access {
        owners: BTreeSet::from([AppName::parse(FILES).expect("name")]),
    };
    serve_storage(&server, drives, known.clone(), apps)
        .await
        .expect("storage1");
    serve(&server, Hub::default(), known.clone())
        .await
        .expect("sync1");
    Rig {
        bus,
        known,
        listing,
        source,
        drive,
        sink,
        _server: server,
    }
}

async fn storage(connection: &zbus::Connection) -> StorageProxy<'_> {
    StorageProxy::new(connection).await.expect("proxy")
}

fn at(path: &str) -> String {
    format!("{ROOT}/{path}")
}

fn text(details: &porter_dbus::Details, key: &str) -> Option<String> {
    details
        .get(key)
        .and_then(|v| String::try_from(v.clone()).ok())
}

fn flag(details: &porter_dbus::Details, key: &str) -> Option<bool> {
    details.get(key).and_then(|v| bool::try_from(v).ok())
}

async fn state_of(proxy: &StorageProxy<'_>, path: &str) -> Option<String> {
    let items = proxy.items(vec![at(path)]).await.expect("items");
    items.get(&at(path)).and_then(|d| text(d, ITEM_KEY_STATE))
}

fn interface_of(xml: &str) -> Vec<String> {
    xml.lines()
        .map(str::trim)
        .skip_while(|l| !l.starts_with("<interface name=\"org.quire.Storage1\""))
        .take_while(|l| *l != "</interface>")
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_served_object_has_exactly_the_frozen_interface() {
    let rig = rig().await;
    let me = client(
        &rig.bus,
        &rig.known,
        "org.quire.Shell",
        CallerRole::SheetHost,
    )
    .await;
    let live = zbus::fdo::IntrospectableProxy::builder(&me)
        .destination(SYNC_BUS)
        .expect("destination")
        .path(STORAGE_PATH)
        .expect("path")
        .build()
        .await
        .expect("proxy")
        .introspect()
        .await
        .expect("introspect");
    let frozen = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dbus/org.quire.Storage1.xml"),
    )
    .expect("the checked-in xml");
    let (served, pinned) = (interface_of(&live), interface_of(&frozen));
    assert!(served.len() > 10, "{live}");
    assert_eq!(served, pinned);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shell_settings_and_an_app_allowed_files_see_the_drives_and_no_one_else() {
    let rig = rig().await;
    let stranger = rig.bus.connect().await;
    let err = storage(&stranger)
        .await
        .drives()
        .await
        .expect_err("unknown");
    assert_eq!(error_name(&err), ACCESS_DENIED);
    let other = client(&rig.bus, &rig.known, "org.example.Other", CallerRole::App).await;
    let other = storage(&other).await;
    for err in [
        other.drives().await.map(drop).expect_err("drives"),
        other
            .items(vec![at("Work")])
            .await
            .map(drop)
            .expect_err("items"),
        other
            .download(vec![at("Work")])
            .await
            .map(drop)
            .expect_err("download"),
        other.watch().await.expect_err("watch"),
    ] {
        assert_eq!(error_name(&err), ACCESS_DENIED);
    }
    for (name, role) in [
        ("org.quire.Shell", CallerRole::SheetHost),
        ("org.quire.Settings", CallerRole::Settings),
        (FILES, CallerRole::App),
    ] {
        let me = client(&rig.bus, &rig.known, name, role).await;
        let drives = storage(&me).await.drives().await.expect("drives");
        assert_eq!(drives.len(), 1, "{name}");
        let (id, details) = &drives[0];
        assert_eq!(id, "one");
        assert_eq!(text(details, DRIVE_KEY_PATH).as_deref(), Some(ROOT));
        assert_eq!(
            text(details, DRIVE_KEY_PROVIDER).as_deref(),
            Some("OneDrive")
        );
        assert_eq!(text(details, DRIVE_KEY_STATE).as_deref(), Some("ready"));
        assert_eq!(
            details
                .get(DRIVE_KEY_LOCAL)
                .and_then(|v| u64::try_from(v).ok()),
            Some(0)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn items_answer_their_state_kind_and_link_and_leave_out_what_is_not_there() {
    let rig = rig().await;
    let me = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let proxy = storage(&me).await;
    let asked = vec![
        at("Work"),
        at("Work/Report.pdf"),
        at("Plan.gdoc"),
        at("Missing.txt"),
        "/home/example/Elsewhere/a.txt".to_owned(),
        ROOT.to_owned(),
    ];
    let items = proxy.items(asked).await.expect("items");
    assert_eq!(items.len(), 3, "{items:?}");
    let report = &items[&at("Work/Report.pdf")];
    assert_eq!(text(report, ITEM_KEY_STATE).as_deref(), Some("cloud"));
    assert_eq!(text(report, ITEM_KEY_KIND).as_deref(), Some("file"));
    assert_eq!(flag(report, ITEM_KEY_KEPT), Some(false));
    assert_eq!(
        text(&items[&at("Work")], ITEM_KEY_KIND).as_deref(),
        Some("folder")
    );
    let plan = &items[&at("Plan.gdoc")];
    assert_eq!(text(plan, ITEM_KEY_KIND).as_deref(), Some("online"));
    assert_eq!(
        text(plan, ITEM_KEY_URL).as_deref(),
        Some("https://docs.example.test/document/d/doc")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn download_keep_and_remove_act_and_refuse_with_the_words_files_shows() {
    let rig = rig().await;
    let me = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let proxy = storage(&me).await;

    let refused = proxy
        .download(vec![at("Work/Report.pdf"), at("Plan.gdoc"), at("Nope")])
        .await
        .expect("download");
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert_eq!(refused[&at("Plan.gdoc")], words::reason(Reason::OnlineOnly));
    assert_eq!(refused[&at("Nope")], words::reason(Reason::NotOnADrive));
    eventually("the report is downloaded", || {
        rig.drive
            .drive
            .resolve("Work/Report.pdf".as_ref())
            .ok()
            .flatten()
            .and_then(|n| rig.drive.drive.status(&n.id))
            .is_some_and(|s| s.state == porter_drive::ItemState::Downloaded)
    })
    .await;
    assert_eq!(
        state_of(&proxy, "Work").await.as_deref(),
        Some("downloaded")
    );

    let refused = proxy.keep(vec![at("Kept")], true).await.expect("keep");
    assert!(refused.is_empty(), "{refused:?}");
    let items = proxy
        .items(vec![at("Kept/Photo.jpg")])
        .await
        .expect("items");
    assert_eq!(
        flag(&items[&at("Kept/Photo.jpg")], ITEM_KEY_KEPT),
        Some(true)
    );
    let refused = proxy
        .remove_download(vec![at("Kept/Photo.jpg"), at("Work/Report.pdf")])
        .await
        .expect("remove");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(
        refused[&at("Kept/Photo.jpg")],
        words::reason(Reason::KeptWithFolder)
    );
    assert_eq!(
        state_of(&proxy, "Work/Report.pdf").await.as_deref(),
        Some("cloud")
    );
    let refused = proxy
        .keep(vec![at("Kept/Photo.jpg")], false)
        .await
        .expect("keep off");
    assert_eq!(
        refused[&at("Kept/Photo.jpg")],
        words::reason(Reason::KeptWithFolder)
    );

    rig.source.set_offline(true);
    rig.listing.file("n", Some("w"), "Notes.txt", 2, "1");
    rig.drive.drive.listing_changed();
    let refused = proxy
        .download(vec![at("Work/Notes.txt")])
        .await
        .expect("download");
    assert!(refused.is_empty() || refused[&at("Work/Notes.txt")] == words::reason(Reason::Offline));
    eventually("offline, it says it downloads when connected", || {
        rig.drive
            .drive
            .resolve("Work/Notes.txt".as_ref())
            .ok()
            .flatten()
            .and_then(|n| rig.drive.drive.status(&n.id))
            .is_some_and(|s| s.reason == Some(Reason::Offline))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_shell_and_settings_pause_a_drive() {
    let rig = rig().await;
    let app = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let err = storage(&app)
        .await
        .set_paused(vec!["one".into()], true)
        .await
        .expect_err("an app");
    assert_eq!(error_name(&err), ACCESS_DENIED);

    let settings = client(
        &rig.bus,
        &rig.known,
        "org.quire.Settings",
        CallerRole::Settings,
    )
    .await;
    let proxy = storage(&settings).await;
    let refused = proxy
        .set_paused(vec!["one".into(), "two".into()], true)
        .await
        .expect("pause");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused["two"], words::reason(Reason::NoDrive));
    let drives = proxy.drives().await.expect("drives");
    assert_eq!(
        text(&drives[0].1, DRIVE_KEY_STATE).as_deref(),
        Some("paused")
    );
    assert!(text(&drives[0].1, DRIVE_KEY_WORDS).is_none());
    proxy
        .set_paused(vec!["one".into()], false)
        .await
        .expect("resume");
    let drives = proxy.drives().await.expect("drives");
    assert_eq!(
        text(&drives[0].1, DRIVE_KEY_STATE).as_deref(),
        Some("ready")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watcher_hears_a_download_move_with_its_folder_and_others_hear_nothing() {
    let rig = rig().await;
    let me = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let proxy = storage(&me).await;
    let mut items = proxy.receive_items_changed().await.expect("stream");
    let mut drives = proxy.receive_drives_changed().await.expect("stream");
    proxy.watch().await.expect("watch");
    let quiet = client(
        &rig.bus,
        &rig.known,
        "org.quire.Shell",
        CallerRole::SheetHost,
    )
    .await;
    let quiet = storage(&quiet).await;
    let mut unheard = quiet.receive_items_changed().await.expect("stream");

    rig.source.hold();
    proxy
        .download(vec![at("Work/Report.pdf")])
        .await
        .expect("download");
    let mut seen_downloading = false;
    let mut seen_folder = false;
    let deadline = test_kit::Deadline::generous();
    loop {
        if deadline.passed() {
            deadline.fail("the download is told");
        }
        rig.source.allow(1);
        let next = std::future::poll_fn(|cx| Pin::new(&mut items).poll_next(cx));
        let Ok(Some(signal)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), next).await
        else {
            continue;
        };
        let args = signal.args().expect("args");
        assert_eq!(args.drive(), &"one");
        let told = args.items();
        if told.contains_key(&at("Work")) {
            seen_folder = true;
        }
        let Some(report) = told.get(&at("Work/Report.pdf")) else {
            continue;
        };
        match text(report, ITEM_KEY_STATE).as_deref() {
            Some("downloading") => {
                seen_downloading = true;
                let fraction: Option<f64> = report
                    .get(ITEM_KEY_FRACTION)
                    .and_then(|v: &OwnedValue| f64::try_from(v).ok());
                assert!(fraction.is_some_and(|f| (0.0..=1.0).contains(&f)));
            }
            Some("downloaded") => break,
            _ => {}
        }
    }
    rig.source.release();
    assert!(seen_downloading, "the fraction moved");
    assert!(seen_folder, "the folder above came with it");
    let next = std::future::poll_fn(|cx| Pin::new(&mut drives).poll_next(cx));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), next)
            .await
            .is_ok(),
        "the bytes here changed"
    );
    let next = std::future::poll_fn(|cx| Pin::new(&mut unheard).poll_next(cx));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), next)
            .await
            .is_err(),
        "a connection that did not watch hears nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn save_a_copy_writes_the_document_next_to_it_and_refuses_what_has_no_file() {
    let rig = rig().await;
    rig.source.put_export("doc", b"docx".to_vec());
    rig.listing.online(
        "form",
        None,
        "Survey",
        "https://docs.example.test/forms/d/form",
        None,
    );
    let me = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let proxy = storage(&me).await;
    let (saved, refused) = proxy
        .save_copy(vec![
            at("Plan.gdoc"),
            at("Survey.glink"),
            at("Work/Report.pdf"),
            at("Nope.gdoc"),
        ])
        .await
        .expect("save copy");
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[&at("Plan.gdoc")], at("Plan.docx"));
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert_eq!(
        refused[&at("Survey.glink")],
        words::reason(Reason::NoFileFormat)
    );
    assert_eq!(
        refused[&at("Work/Report.pdf")],
        words::reason(Reason::NotADocument)
    );
    assert_eq!(
        refused[&at("Nope.gdoc")],
        words::reason(Reason::NotOnADrive)
    );
    let items = proxy.items(vec![at("Plan.docx")]).await.expect("items");
    let copy = &items[&at("Plan.docx")];
    assert_eq!(text(copy, ITEM_KEY_STATE).as_deref(), Some("waiting"));
    assert_eq!(text(copy, ITEM_KEY_KIND).as_deref(), Some("file"));
    let (saved, _) = proxy.save_copy(vec![at("Plan.gdoc")]).await.expect("again");
    assert_eq!(saved[&at("Plan.gdoc")], at("Plan 2.docx"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_made_here_waits_while_paused_then_uploads_and_is_downloaded_once_sent() {
    let rig = writable().await;
    let me = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let proxy = storage(&me).await;
    rig.drive.set_paused(true);
    let files = &rig.drive.drive;
    let work = files
        .resolve("Work".as_ref())
        .expect("resolve")
        .expect("there")
        .id;
    let (id, handle) = files.create(&work, "Notes.txt").expect("create");
    files.write(handle, 0, b"notes").await.expect("write");
    files.release_write(handle);
    let (_, lock) = files.create(&work, ".~lock.Notes.txt#").expect("lock");
    files.release_write(lock);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        state_of(&proxy, "Work/Notes.txt").await.as_deref(),
        Some("waiting")
    );
    assert_eq!(state_of(&proxy, "Work").await.as_deref(), Some("waiting"));
    let items = proxy
        .items(vec![at("Work/.~lock.Notes.txt#")])
        .await
        .expect("items");
    assert!(
        items.is_empty(),
        "a lock file is not the drive's: {items:?}"
    );
    assert_eq!(rig.sink.took(), [], "held while paused");

    rig.sink.hold();
    rig.drive.set_paused(false);
    eventually("uploading", || {
        files
            .status(&id)
            .is_some_and(|s| s.state == porter_drive::ItemState::Uploading)
    })
    .await;
    let items = proxy
        .items(vec![at("Work/Notes.txt")])
        .await
        .expect("items");
    let notes = &items[&at("Work/Notes.txt")];
    assert_eq!(text(notes, ITEM_KEY_STATE).as_deref(), Some("uploading"));
    let fraction = notes
        .get(ITEM_KEY_FRACTION)
        .and_then(|v| f64::try_from(v).ok())
        .expect("a fraction");
    assert!(fraction > 0.0 && fraction < 1.0, "{fraction}");
    assert_eq!(state_of(&proxy, "Work").await.as_deref(), Some("uploading"));
    rig.sink.release();
    eventually("sent", || !rig.sink.took().is_empty()).await;
    assert_eq!(
        rig.sink.took(),
        [Took::New(Some("w".into()), "Notes.txt".into(), 5)]
    );
    eventually("downloaded once sent", || {
        matches!(files.canonical(&id), NodeId::Remote(_))
            && files
                .status(&id)
                .is_some_and(|s| s.state == porter_drive::ItemState::Downloaded)
    })
    .await;
    assert_eq!(
        state_of(&proxy, "Work/Notes.txt").await.as_deref(),
        Some("downloaded")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_changed_here_and_on_the_drive_shows_both_in_conflict_each_naming_the_other() {
    let rig = writable().await;
    let me = client(&rig.bus, &rig.known, FILES, CallerRole::App).await;
    let proxy = storage(&me).await;
    rig.drive.set_paused(true);
    let files = &rig.drive.drive;
    let report = NodeId::Remote(porter_sync::RemoteId("r".into()));
    let handle = files.open_write(&report, true).await.expect("open");
    files.write(handle, 0, b"mine").await.expect("write");
    files.release_write(handle);
    rig.listing.file("r", Some("w"), "Report.pdf", 6, "2");
    rig.source.put("r", b"theirs".to_vec());
    files.listing_changed();

    let mine = at("Work/Report (this computer).pdf");
    let theirs = at("Work/Report.pdf");
    let items = proxy
        .items(vec![mine.clone(), theirs.clone()])
        .await
        .expect("items");
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(
        text(&items[&mine], ITEM_KEY_STATE).as_deref(),
        Some("conflict")
    );
    assert_eq!(
        text(&items[&theirs], ITEM_KEY_STATE).as_deref(),
        Some("conflict")
    );
    assert_eq!(
        text(&items[&mine], ITEM_KEY_OTHER).as_deref(),
        Some(theirs.as_str())
    );
    assert_eq!(
        text(&items[&theirs], ITEM_KEY_OTHER).as_deref(),
        Some(mine.as_str())
    );
    assert_eq!(state_of(&proxy, "Work").await.as_deref(), Some("conflict"));
    let refused = proxy
        .remove_download(vec![mine.clone()])
        .await
        .expect("remove");
    assert_eq!(refused[&mine], words::reason(Reason::Conflict));
}
