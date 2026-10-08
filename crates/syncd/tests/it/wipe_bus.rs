//! `AccountRemoved` on a private bus with a fake accountd: syncd introduces itself to accountd,
//! then wipes that account's journals, anchors and mirrors and stops its datasets when the
//! signal comes, and only for accountd's signal.

use crate::common;

use common::bus::PrivateBus;
use common::eventually;
use porter_dbus::{ACCOUNTS_BUS, ACCOUNTS_PATH, Details};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use syncd::paths::{AccountDir, Paths};
use syncd::removal;
use syncd::service::{Access, DatasetName, Hub};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::ObjectPath;

/// accountd as far as syncd can tell: it answers `Grants.List` (so it hears who calls) and can
/// emit `AccountRemoved`.
#[derive(Debug, Clone, Default)]
struct FakeGrants(Arc<Mutex<Vec<String>>>);

#[zbus::interface(name = "org.quire.Accounts1.Grants")]
impl FakeGrants {
    async fn list(&self, #[zbus(header)] header: Header<'_>) -> Vec<(String, Details)> {
        let sender = header.sender().map(ToString::to_string).unwrap_or_default();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sender);
        Vec::new()
    }
}

impl FakeGrants {
    fn heard(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

async fn fake_accountd(bus: &PrivateBus) -> (zbus::Connection, FakeGrants) {
    let connection = bus.connect().await;
    let grants = FakeGrants::default();
    connection
        .object_server()
        .at(ACCOUNTS_PATH, grants.clone())
        .await
        .expect("serve Grants");
    // As a daemon does: the name only once the connection answers calls.
    porter_dbus::serve_ready(&connection)
        .await
        .expect("the fake takes calls");
    connection
        .request_name(ACCOUNTS_BUS)
        .await
        .expect("own the name");
    (connection, grants)
}

fn scratch_paths(bus: &PrivateBus) -> Paths {
    let root = bus.scratch().join("syncd");
    Paths {
        journals: root.join("state/porter/sync"),
        mirrors: root.join("data/porter/vdir"),
        callers_system: root.join("none"),
        callers_user: root.join("none"),
    }
}

/// A journal file and a mirrored event for the account.
fn seed(paths: &Paths, account: &str) -> [PathBuf; 2] {
    let account = AccountDir::parse(account).expect("segment");
    let journal = paths.journal(&account, "pim");
    std::fs::create_dir_all(journal.parent().expect("dir")).expect("journal dir");
    std::fs::write(&journal, "journal rows and anchors").expect("journal");
    let [_, mirror] = paths.account_dirs(&account);
    std::fs::create_dir_all(mirror.join("personal")).expect("mirror dir");
    std::fs::write(mirror.join("personal/event.ics"), "BEGIN:VEVENT").expect("mirror");
    paths.account_dirs(&account)
}

fn gone(dirs: &[PathBuf; 2]) -> bool {
    dirs.iter().all(|d| !d.exists())
}

fn present(dirs: &[PathBuf; 2]) -> bool {
    dirs.iter().all(|d: &PathBuf| Path::exists(d))
}

/// `AccountRemoved` for `account`, sent by `from` to `to` (a unique name; all when empty).
async fn removed(from: &zbus::Connection, to: &str, account: &str) {
    let path = ObjectPath::try_from(format!("{ACCOUNTS_PATH}/account/{account}")).expect("path");
    let destination = (!to.is_empty()).then(|| BusName::try_from(to.to_owned()).expect("name"));
    from.emit_signal(
        destination,
        ACCOUNTS_PATH,
        "org.quire.Accounts1.Manager",
        "AccountRemoved",
        &(path,),
    )
    .await
    .expect("emit");
}

#[tokio::test(flavor = "multi_thread")]
async fn accountd_telling_syncd_an_account_is_gone_wipes_its_journals_mirrors_and_datasets() {
    let bus = PrivateBus::start();
    let (accountd, grants) = fake_accountd(&bus).await;
    let paths = scratch_paths(&bus);
    let (a1, a2) = (seed(&paths, "a1"), seed(&paths, "a2"));
    let hub = Hub::default();
    let dataset = DatasetName::parse("a1/pim").expect("name");
    let handle = hub.register(dataset, Access::default());

    let syncd = bus.connect().await;
    let unique = syncd.unique_name().expect("name").to_string();
    removal::watch(&syncd, hub.clone(), paths.clone())
        .await
        .expect("watch");
    eventually("accountd hears syncd", || grants.heard().contains(&unique)).await;

    // Unicast, as accountd sends it.
    removed(&accountd, &unique, "a1").await;
    eventually("a1 is wiped", || gone(&a1)).await;
    assert!(present(&a2), "another account is untouched");
    assert!(!handle.is_registered(), "its datasets are stopped");

    // A broadcast of the same signal by accountd counts too; a repeat is harmless.
    removed(&accountd, "", "a1").await;
    removed(&accountd, "", "a2").await;
    eventually("a2 is wiped", || gone(&a2)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signal_from_anyone_but_accountd_wipes_nothing() {
    let bus = PrivateBus::start();
    let (accountd, grants) = fake_accountd(&bus).await;
    let paths = scratch_paths(&bus);
    let a1 = seed(&paths, "a1");
    let a2 = seed(&paths, "a2");
    let hub = Hub::default();
    let syncd = bus.connect().await;
    let unique = syncd.unique_name().expect("name").to_string();
    removal::watch(&syncd, hub.clone(), paths.clone())
        .await
        .expect("watch");
    eventually("accountd hears syncd", || grants.heard().contains(&unique)).await;

    // An app that does not own org.quire.Accounts1 forges the signal, to syncd and to all.
    let forger = bus.connect().await;
    removed(&forger, &unique, "a1").await;
    removed(&forger, "", "a1").await;
    // A sentinel from the real accountd proves the listener is alive and had time to act on
    // the forgeries.
    removed(&accountd, &unique, "a2").await;
    eventually("the real signal is acted on", || gone(&a2)).await;
    assert!(present(&a1), "the forged signals changed nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn syncd_introduces_itself_again_when_accountd_restarts() {
    let bus = PrivateBus::start();
    let (first, grants) = fake_accountd(&bus).await;
    let paths = scratch_paths(&bus);
    let syncd = bus.connect().await;
    let unique = syncd.unique_name().expect("name").to_string();
    removal::watch(&syncd, Hub::default(), paths)
        .await
        .expect("watch");
    eventually("introduced once", || {
        grants.heard().iter().filter(|s| **s == unique).count() == 1
    })
    .await;
    drop(first);
    let (_second, grants_again) = fake_accountd(&bus).await;
    eventually("introduced to the new accountd", || {
        grants_again.heard().contains(&unique)
    })
    .await;
}
