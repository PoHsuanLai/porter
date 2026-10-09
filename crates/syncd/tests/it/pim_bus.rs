//! The PIM mirror end to end on one private bus: accountd (the real service over in-memory
//! secrets) holds the app password and runs the relay; syncd's supervisor, holding a Calendar
//! and a Contacts grant, discovers the collections of a fake Nextcloud through the relay and
//! mirrors each into a vdir under a scratch `$XDG_DATA_HOME`. Nothing here touches the real
//! session bus, `~/.local` or the network.

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::caller as caller_of;
use porter_client::{Accounts, DbusTransport};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, CapabilityKind,
    Claim, Credential, DataClass, EndpointUrl, Family, GrantId, Isolation, LoginName, Offer,
    Provenance, ProviderId, Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint,
    SpaceScope, Subject, Tls, UnixSeconds,
};
use porter_dbus::{Caller, CallerRole};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::{FakeNextcloud, NextcloudHandle, Running};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use syncd::datasets::pim::{
    AccountdUnavailable, COLOR, ClientGrants, DISPLAYNAME, PimConfig, PimGrants, PimKind,
    PimSupervisor, Wiring, is_complete, items_in,
};
use syncd::paths::Paths;
use syncd::removal;
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access, Hub};
use tokio::sync::watch;

const PASSWORD: &str = "S3CRET-PIM-APP-PASSWORD";
const SYNCD: &str = "org.quire.Sync";
const ACCOUNT: &str = "pim-cloud";
const SEGMENT: &str = "pim_cloud";
const PROVIDER: &str = r#"
id = "pim-cloud"
label = "PIM Cloud"
mark = "generic"
[auth]
kind = "app_password"
[discovery]
kind = "fixed"
[[capability]]
family = "caldav"
kind = "calendar"
v = { access = "read_write", delta = "poll", transport = "caldav", collections = "present" }
[[capability]]
family = "carddav"
kind = "contacts"
v = { access = "read_write", delta = "poll", transport = "carddav", collections = "present" }
"#;

fn syncd_app() -> AppId {
    AppId {
        name: AppName::parse(SYNCD).expect("app"),
        isolation: Isolation::Flatpak,
    }
}

fn event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20261006T100000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn card(uid: &str, name: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nEND:VCARD\r\n")
}

/// Polls every 20 ms for up to [`porter_fake::GENEROUS`] (a poll cycle is a second here; a
/// loaded machine needs the margin, a passing check returns at once).
async fn eventually(what: &str, check: impl FnMut() -> bool) {
    eventually_within(porter_fake::GENEROUS, what, check).await;
}

/// Polls every 20 ms for up to `limit`: for waits whose work grows with a loaded machine (twenty
/// large items a round), which ten seconds does not cover when every core is busy.
async fn eventually_within(limit: Duration, what: &str, mut check: impl FnMut() -> bool) {
    let deadline = porter_fake::Deadline::after(limit);
    while !deadline.passed() {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

/// Tells a blocking helper to stop when the test ends, by success or by panic: the runtime waits
/// for its blocking threads when it drops, so a helper still looping after a panic hangs the test
/// instead of failing it.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn read(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// `accounts.grants` as the supervisor sees accountd, which a test can make unreachable.
struct Switch {
    inner: ClientGrants<DbusTransport>,
    up: Arc<AtomicBool>,
}

impl PimGrants for Switch {
    async fn granted(
        &self,
        kind: PimKind,
    ) -> Result<Vec<porter_core::Candidate>, AccountdUnavailable> {
        match self.up.load(Ordering::Relaxed) {
            true => self.inner.granted(kind).await,
            false => Err(AccountdUnavailable),
        }
    }
}

type Remove = Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

struct Rig {
    bus: PrivateBus,
    nextcloud: Running<NextcloudHandle>,
    hub: Hub,
    mirrors: PathBuf,
    accounts: Arc<Accounts<DbusTransport>>,
    remove_account: Remove,
    accountd_up: Arc<AtomicBool>,
    grants: Vec<GrantId>,
    supervisor: tokio::task::JoinHandle<()>,
    _network: watch::Sender<Network>,
    _keep: Vec<zbus::Connection>,
}

impl Rig {
    fn account_dir(&self) -> PathBuf {
        self.mirrors.join(SEGMENT)
    }

    fn calendar(&self, name: &str) -> PathBuf {
        self.account_dir().join(name)
    }

    fn requests(&self, method: &str) -> usize {
        self.nextcloud
            .hits()
            .iter()
            .filter(|h| h.method == method && h.target.contains("/calendars/"))
            .count()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.supervisor.abort();
    }
}

fn grant_for(kind: CapabilityKind, class: DataClass, id: &str) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("grant"),
        key: GrantKey {
            app: syncd_app(),
            account: AccountId::parse(ACCOUNT).expect("account"),
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

async fn rig(with_grants: bool) -> Rig {
    let nextcloud = FakeNextcloud::start("alice").await.expect("nextcloud");
    nextcloud.seed_app_password(PASSWORD);
    nextcloud.delete_collection("tasks");
    nextcloud.add_calendar("work", "VEVENT", "Work", "#ff0000FF");
    nextcloud.set_collection_meta("personal", "Personal", Some("#0082c9FF"));
    nextcloud.put_item("personal", "ev1.ics", &event("ev1", "Dentist"));
    nextcloud.put_item("personal", "ev2.ics", &event("ev2", "Lunch"));
    nextcloud.put_item("work", "w1.ics", &event("w1", "Standup"));
    nextcloud.put_item("contacts", "c1.vcf", &card("c1", "Ada Lovelace"));
    nextcloud.put_item("contacts", "c2.vcf", &card("c2", "Grace Hopper"));
    let dav = format!("{}/remote.php/dav/", nextcloud.base_url());

    let spec = FakeProvider::from_file(PROVIDER);
    let endpoint = |family| ServiceEndpoint {
        family,
        url: EndpointUrl::parse(&dav).expect("url"),
        tls: Tls::Plain,
        login: LoginName("alice".into()),
    };
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: ProviderId::parse(ACCOUNT).expect("provider"),
        label: AccountLabel("alice@cloud.invalid".into()),
        state: AccountState::Ok,
        auth: AuthKind::AppPassword,
        capabilities: spec
            .spec()
            .capabilities
            .iter()
            .map(|row| Claim {
                subject: Subject::Account,
                offer: Offer::Present(row.capability.clone()),
                provenance: Provenance::Declared,
            })
            .collect(),
        restriction: Restriction::none(),
        endpoints: vec![endpoint(Family::CalDav), endpoint(Family::CardDav)],
    };
    let granted = [
        grant_for(CapabilityKind::Calendar, DataClass::Calendar, "pim-cal"),
        grant_for(CapabilityKind::Contacts, DataClass::Contacts, "pim-card"),
    ];
    let grants: Vec<GrantId> = granted.iter().map(|g| g.id.clone()).collect();
    let secrets = MemorySecrets::default();
    secrets
        .put(
            &SecretKey {
                account: AccountId::parse(ACCOUNT).expect("id"),
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
            vec![spec],
            Registry {
                grants: if with_grants {
                    granted.to_vec()
                } else {
                    vec![]
                },
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
    let removing = Arc::clone(&service);
    let remove_account: Remove = Box::new(move || {
        let service = Arc::clone(&removing);
        Box::pin(async move {
            service
                .remove_account(&AccountId::parse(ACCOUNT).expect("id"))
                .await
                .expect("removed");
        })
    });
    serve_with(&accountd, service, Arc::clone(&table), Options::default())
        .await
        .expect("accountd serves");
    let syncd_side = bus.connect().await;
    table.introduce_as(
        syncd_side.unique_name().expect("name").as_str(),
        Caller {
            app: syncd_app(),
            role: CallerRole::App,
        },
    );
    let accounts = Arc::new(Accounts::over(DbusTransport::over(syncd_side.clone())));

    let root = bus.scratch().join("syncd");
    let paths = Paths {
        journals: root.join("state/porter/sync"),
        mirrors: root.join("data/porter/vdir"),
        callers_system: root.join("none"),
        callers_user: root.join("none"),
    };
    let hub = Hub::default();
    removal::watch(&syncd_side, hub.clone(), paths.clone())
        .await
        .expect("watch AccountRemoved");
    let (network_up, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: Arc::clone(&accounts),
        hub: hub.clone(),
        paths: paths.clone(),
        settings: Settings {
            poll_base: 1,
            poll_max: 1,
            push_window: 0,
            batch_window: 0,
            metered: MeteredPolicy::Pause,
        },
        network,
        owners: Access::default(),
    };
    let accountd_up = Arc::new(AtomicBool::new(true));
    let supervisor = PimSupervisor::new(
        wiring,
        Switch {
            inner: ClientGrants::new(Arc::clone(&accounts)),
            up: Arc::clone(&accountd_up),
        },
        PimConfig {
            rescan: Duration::from_millis(300),
        },
    )
    .spawn();
    Rig {
        bus,
        nextcloud,
        hub,
        mirrors: paths.mirrors,
        accounts,
        remove_account,
        accountd_up,
        grants,
        supervisor,
        _network: network_up,
        _keep: vec![accountd, syncd_side],
    }
}

fn settings_caller() -> Caller {
    caller_of("org.quire.Settings", CallerRole::Settings)
}

fn names(rig: &Rig) -> Vec<String> {
    let mut all = rig.hub.names_for(&settings_caller());
    all.sort();
    all
}

#[tokio::test(flavor = "multi_thread")]
async fn two_calendars_and_an_address_book_mirror_into_the_vdir_with_their_metadata() {
    let rig = rig(true).await;
    let personal = rig.calendar("personal");
    let work = rig.calendar("work");
    let contacts = rig.calendar("contacts-contacts");
    eventually("every item is mirrored", || {
        items_in(&personal, PimKind::Calendar) == ["ev1.ics", "ev2.ics"]
            && items_in(&work, PimKind::Calendar) == ["w1.ics"]
            && items_in(&contacts, PimKind::Contacts) == ["c1.vcf", "c2.vcf"]
    })
    .await;

    // The files are the server's, byte for byte, named by UID.
    assert_eq!(
        read(&personal.join("ev1.ics")),
        Some(event("ev1", "Dentist"))
    );
    assert_eq!(read(&work.join("w1.ics")), Some(event("w1", "Standup")));
    assert_eq!(
        read(&contacts.join("c2.vcf")),
        Some(card("c2", "Grace Hopper"))
    );
    // The metadata is vdirsyncer's two files.
    eventually("the metadata is written", || {
        read(&personal.join(DISPLAYNAME)).as_deref() == Some("Personal")
            && read(&personal.join(COLOR)).as_deref() == Some("#0082c9FF")
            && read(&work.join(DISPLAYNAME)).as_deref() == Some("Work")
            && read(&work.join(COLOR)).as_deref() == Some("#ff0000FF")
            && read(&contacts.join(DISPLAYNAME)).as_deref() == Some("contacts")
    })
    .await;
    assert!(
        !contacts.join(COLOR).exists(),
        "an address book has no colour"
    );
    let mut collections: Vec<String> = std::fs::read_dir(rig.account_dir())
        .expect("account dir")
        .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
        .collect();
    collections.sort();
    assert_eq!(collections, ["contacts-contacts", "personal", "work"]);

    // Sync1 names them <account>/<dataset>.
    let mut want = vec![
        format!("{SEGMENT}/pim_card_contacts_contacts"),
        format!("{SEGMENT}/pim_cal_personal"),
        format!("{SEGMENT}/pim_cal_work"),
    ];
    want.sort();
    assert_eq!(names(&rig), want);

    // The relay, not syncd, authenticated: every DAV request carries the app password's Basic
    // header, which syncd never held.
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
    assert!(dav.len() > 5);
    assert!(
        dav.iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );
    // Read-only: nothing was written to the server.
    assert!(
        dav.iter()
            .all(|h| !matches!(h.method.as_str(), "PUT" | "DELETE" | "MKCOL")),
        "{:?}",
        dav.iter()
            .map(|h| (&h.method, &h.target))
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_edit_a_delete_a_new_item_a_rename_and_collection_changes_all_arrive() {
    let rig = rig(true).await;
    let personal = rig.calendar("personal");
    eventually("the first mirror", || {
        items_in(&personal, PimKind::Calendar) == ["ev1.ics", "ev2.ics"]
    })
    .await;

    rig.nextcloud
        .put_item("personal", "ev1.ics", &event("ev1", "Dentist (moved)"));
    rig.nextcloud.delete_item("personal", "ev2.ics");
    // A file name that is not its UID: the mirror names it by the UID.
    rig.nextcloud
        .put_item("personal", "x-server-name.ics", &event("real-uid", "Trip"));
    eventually("edit, delete and new item arrive", || {
        read(&personal.join("ev1.ics")) == Some(event("ev1", "Dentist (moved)"))
            && items_in(&personal, PimKind::Calendar) == ["ev1.ics", "real-uid.ics"]
    })
    .await;
    assert_eq!(
        read(&personal.join("real-uid.ics")),
        Some(event("real-uid", "Trip"))
    );

    // Renamed and recoloured on the server; a new calendar; a calendar deleted.
    rig.nextcloud
        .set_collection_meta("personal", "Private", Some("#00ff00FF"));
    rig.nextcloud
        .add_calendar("holidays", "VEVENT", "Holidays", "#123456FF");
    rig.nextcloud
        .put_item("holidays", "h1.ics", &event("h1", "Midsummer"));
    rig.nextcloud.delete_collection("work");
    let holidays = rig.calendar("holidays");
    eventually("metadata, new calendar and the removal arrive", || {
        read(&personal.join(DISPLAYNAME)).as_deref() == Some("Private")
            && read(&personal.join(COLOR)).as_deref() == Some("#00ff00FF")
            && items_in(&holidays, PimKind::Calendar) == ["h1.ics"]
            && read(&holidays.join(DISPLAYNAME)).as_deref() == Some("Holidays")
            && !rig.calendar("work").exists()
    })
    .await;
    assert!(
        !names(&rig).iter().any(|n| n.ends_with("pim_cal_work")),
        "{:?}",
        names(&rig)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_sync_token_lists_again_and_downloads_nothing_it_already_has() {
    let rig = rig(true).await;
    let personal = rig.calendar("personal");
    eventually("the first mirror", || {
        items_in(&personal, PimKind::Calendar) == ["ev1.ics", "ev2.ics"]
            && items_in(&rig.calendar("work"), PimKind::Calendar) == ["w1.ics"]
            && items_in(&rig.calendar("contacts-contacts"), PimKind::Contacts).len() == 2
    })
    .await;
    // Let the cycles that follow the first settle, then count what was downloaded so far.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let gets = rig.requests("GET");
    assert_eq!(gets, 3, "each calendar item exactly once");

    // A change moves the server's counter past every token the mirrors hold; expiring tokens
    // right after it makes each of them stale (a token as new as the counter would stay good).
    rig.nextcloud
        .put_item("personal", "ev3.ics", &event("ev3", "Gym"));
    rig.nextcloud.expire_sync_tokens();
    eventually("the new item arrives", || personal.join("ev3.ics").exists()).await;
    // Every collection was listed again (a REPORT with no token), and only the new item came.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        rig.requests("GET"),
        gets + 1,
        "no unchanged item was downloaded again"
    );
    let reports = rig
        .nextcloud
        .hits()
        .iter()
        .filter(|h| h.method == "REPORT" && h.status == 403)
        .count();
    assert!(reports >= 3, "every collection met the expiry: {reports}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_edit_a_deletion_and_a_new_file_are_never_sent_to_the_server() {
    let rig = rig(true).await;
    let personal = rig.calendar("personal");
    eventually("the first mirror", || {
        items_in(&personal, PimKind::Calendar) == ["ev1.ics", "ev2.ics"]
    })
    .await;
    std::fs::write(personal.join("ev1.ics"), "locally edited").expect("edit");
    std::fs::remove_file(personal.join("ev2.ics")).expect("delete");
    std::fs::write(personal.join("mine.ics"), event("mine", "Local only")).expect("new file");
    // Three poll cycles pass.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let writes: Vec<_> = rig
        .nextcloud
        .hits()
        .into_iter()
        .filter(|h| matches!(h.method.as_str(), "PUT" | "DELETE" | "MKCOL"))
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
    // Pull-only: the local state stays until the server next changes those items, which then
    // overwrite it (no conflict); the stray file is left alone and never uploaded.
    assert_eq!(
        read(&personal.join("ev1.ics")).as_deref(),
        Some("locally edited")
    );
    assert!(!personal.join("ev2.ics").exists());
    rig.nextcloud
        .put_item("personal", "ev1.ics", &event("ev1", "Dentist (moved)"));
    rig.nextcloud
        .put_item("personal", "ev2.ics", &event("ev2", "Brunch"));
    eventually("the server's state replaces the local one", || {
        read(&personal.join("ev1.ics")) == Some(event("ev1", "Dentist (moved)"))
            && read(&personal.join("ev2.ics")) == Some(event("ev2", "Brunch"))
    })
    .await;
    assert!(personal.join("mine.ics").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_never_sees_a_file_that_is_not_a_complete_item() {
    let rig = rig(true).await;
    let personal = rig.calendar("personal");
    let big = |round: u32, n: u32| {
        let pad = "X-PAD:0123456789abcdef0123456789abcdef0123456789abcdef\r\n".repeat(4000);
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:big{n}\r\nSUMMARY:round-{round}\r\n{pad}END:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    };
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_panic = StopOnDrop(Arc::clone(&stop));
    let reader = {
        let (dir, stop) = (personal.clone(), Arc::clone(&stop));
        tokio::task::spawn_blocking(move || {
            let (mut reads, mut torn) = (0_u64, Vec::new());
            while !stop.load(Ordering::Relaxed) {
                for name in items_in(&dir, PimKind::Calendar) {
                    if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                        reads += 1;
                        if !is_complete(PimKind::Calendar, &bytes) {
                            torn.push((name, bytes.len()));
                        }
                    }
                }
                // The metadata files are small whole writes too.
                for meta in [DISPLAYNAME, COLOR] {
                    if std::fs::read_to_string(dir.join(meta)).is_ok_and(|text| text.is_empty()) {
                        torn.push((meta.to_owned(), 0));
                    }
                }
            }
            (reads, torn)
        })
    };
    for round in 1..=3_u32 {
        for n in 0..20 {
            rig.nextcloud
                .put_item("personal", &format!("big{n}.ics"), &big(round, n));
        }
        let marker = format!("round-{round}");
        eventually_within(Duration::from_secs(90), "the round is mirrored", || {
            (0..20).all(|n| {
                read(&personal.join(format!("big{n}.ics"))).is_some_and(|t| t.contains(&marker))
            })
        })
        .await;
    }
    stop.store(true, Ordering::Relaxed);
    let (reads, torn) = reader.await.expect("reader");
    assert!(
        reads > 200,
        "the reader really ran alongside the writes: {reads}"
    );
    assert!(torn.is_empty(), "torn reads: {torn:?}");
    let leftovers: Vec<_> = std::fs::read_dir(&personal)
        .expect("dir")
        .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn account_removed_removes_the_mirror_and_it_does_not_come_back() {
    let rig = rig(true).await;
    eventually("mirrored", || {
        items_in(&rig.calendar("personal"), PimKind::Calendar).len() == 2
            && items_in(&rig.calendar("contacts-contacts"), PimKind::Contacts).len() == 2
    })
    .await;
    assert!(rig.account_dir().exists());
    assert_eq!(names(&rig).len(), 3);

    (rig.remove_account)().await;
    // accountd tells syncd on its next answer (any call publishes what changed).
    let poke = rig.accounts.grants().await;
    drop(poke);
    eventually("the account's mirror is gone", || {
        !rig.account_dir().exists()
    })
    .await;
    assert!(names(&rig).is_empty(), "{:?}", names(&rig));
    // The supervisor sees no grant any more and starts nothing again.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!rig.account_dir().exists());
    assert!(names(&rig).is_empty());
    let journals = rig
        .bus
        .scratch()
        .join("syncd/state/porter/sync")
        .join(SEGMENT);
    assert!(!journals.exists(), "its journals went with it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_grant_stops_and_removes_the_mirror_but_an_unreachable_accountd_changes_nothing()
{
    let rig = rig(true).await;
    let contacts = rig.calendar("contacts-contacts");
    eventually("mirrored", || {
        items_in(&rig.calendar("personal"), PimKind::Calendar).len() == 2
            && items_in(&contacts, PimKind::Contacts).len() == 2
    })
    .await;

    // accountd cannot be asked: nothing is stopped or removed, and mirroring goes on.
    rig.accountd_up.store(false, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(names(&rig).len(), 3);
    rig.nextcloud
        .put_item("contacts", "c3.vcf", &card("c3", "Alan Turing"));
    eventually("a running mirror still follows the server", || {
        items_in(&contacts, PimKind::Contacts).len() == 3
    })
    .await;

    // The person withdraws the Contacts grant: that mirror goes, the calendars stay.
    rig.accountd_up.store(true, Ordering::Relaxed);
    rig.accounts.revoke(&rig.grants[1]).await.expect("revoke");
    eventually("the address book is gone", || !contacts.exists()).await;
    assert!(rig.calendar("personal").exists());
    assert!(
        names(&rig).iter().all(|n| n.contains("pim_cal_")),
        "{:?}",
        names(&rig)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_grant_nothing_is_mirrored_and_the_server_is_not_even_asked() {
    let rig = rig(false).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(!rig.account_dir().exists());
    assert!(names(&rig).is_empty());
    assert_eq!(rig.requests("REPORT") + rig.requests("PROPFIND"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_calendar_and_an_address_book_of_one_name_keep_apart_and_retiring_one_leaves_the_other() {
    let rig = rig(true).await;
    rig.nextcloud
        .add_calendar("shared", "VEVENT", "Shared", "#111111FF");
    rig.nextcloud
        .put_item("shared", "s1.ics", &event("s1", "Offsite"));
    rig.nextcloud.add_addressbook("shared", "Shared people");
    rig.nextcloud
        .put_book_item("shared", "p1.vcf", &card("p1", "Katherine Johnson"));
    let (calendar, book) = (rig.calendar("shared"), rig.calendar("shared-contacts"));
    eventually("both mirror, each in its own directory", || {
        items_in(&calendar, PimKind::Calendar) == ["s1.ics"]
            && items_in(&book, PimKind::Contacts) == ["p1.vcf"]
    })
    .await;
    assert!(items_in(&calendar, PimKind::Contacts).is_empty());
    assert!(items_in(&book, PimKind::Calendar).is_empty());

    // The Contacts grant is withdrawn: the address book goes, the calendar of that name stays.
    rig.accounts.revoke(&rig.grants[1]).await.expect("revoke");
    eventually("the address book is gone", || !book.exists()).await;
    assert_eq!(items_in(&calendar, PimKind::Calendar), ["s1.ics"]);
    assert_eq!(read(&calendar.join(DISPLAYNAME)).as_deref(), Some("Shared"));
}
