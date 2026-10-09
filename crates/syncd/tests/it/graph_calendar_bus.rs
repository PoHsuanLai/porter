//! A Microsoft calendar grant mirrored into the vdir, on one private bus: accountd (the real
//! service over in-memory secrets, a fake provider that mints a token for the endpoint's
//! audience) holds the OAuth credential and runs the relay, which adds the bearer; syncd's
//! supervisor, holding a Calendar grant whose capability's transport is `graph`, lists the
//! calendars of the rig's `FakeGraph` and mirrors each calendar's `events/delta` as `.ics` files
//! under a scratch `$XDG_DATA_HOME`. Nothing here touches the real session bus, `~/.local` or
//! the network.

use crate::common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::caller as caller_of;
use porter_client::{Accounts, DbusTransport};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, CapabilityKind,
    Claim, Credential, DataClass, EndpointUrl, Family, GrantId, Isolation, LoginName, Offer,
    Provenance, Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, SpaceScope,
    Subject, Tls, UnixSeconds,
};
use porter_dbus::{Caller, CallerRole};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::{FakeGraph, GraphHandle, Running};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use syncd::datasets::pim::{
    COLOR, ClientGrants, DISPLAYNAME, PimConfig, PimKind, PimSupervisor, Wiring, is_complete,
    items_in,
};
use syncd::paths::Paths;
use syncd::scheduler::{Network, Settings};
use syncd::service::{Access, Hub};
use tokio::sync::watch;

const SYNCD: &str = "org.quire.Sync";
const ACCOUNT: &str = "ms-cal";
const SEGMENT: &str = "ms_cal";
/// What the fake provider mints for the audience of this account's endpoint (its family's slug).
const BEARER: &str = "fake:ms-cal:graph";
const PROVIDER: &str = r#"
id = "fake-ms"
label = "Fake Microsoft"
mark = "generic"
[auth]
kind = "oauth_pkce"
issuer = "microsoft"
[discovery]
kind = "autoconfig"
[[capability]]
family = "graph"
kind = "calendar"
v = { access = "read_write", delta = "poll", transport = "graph", collections = "present" }
"#;

fn syncd_app() -> AppId {
    AppId {
        name: AppName::parse(SYNCD).expect("app"),
        isolation: Isolation::Flatpak,
    }
}

/// Polls every 20 ms for up to [`porter_fake::GENEROUS`] by the clock (a poll cycle is a second
/// here).
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    deadline.fail(what);
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

struct Rig {
    _bus: PrivateBus,
    graph: Running<GraphHandle>,
    hub: Hub,
    mirrors: PathBuf,
    supervisor: tokio::task::JoinHandle<()>,
    _network: watch::Sender<Network>,
    _keep: Vec<zbus::Connection>,
}

impl Rig {
    fn account_dir(&self) -> PathBuf {
        self.mirrors.join(SEGMENT)
    }

    /// The directory of the calendar the server calls `name` (directories are named by a hash of
    /// the calendar's id, so a test finds one by its `displayname` file).
    fn calendar(&self, name: &str) -> Option<PathBuf> {
        std::fs::read_dir(self.account_dir())
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|dir| read(&dir.join(DISPLAYNAME)).as_deref() == Some(name))
    }

    fn items(&self, name: &str) -> Vec<String> {
        self.calendar(name)
            .map(|dir| items_in(&dir, PimKind::Calendar))
            .unwrap_or_default()
    }

    /// The text of the item of calendar `name` that holds `needle`.
    fn item_with(&self, name: &str, needle: &str) -> Option<String> {
        let dir = self.calendar(name)?;
        items_in(&dir, PimKind::Calendar)
            .into_iter()
            .filter_map(|file| read(&dir.join(file)))
            .find(|text| text.contains(needle))
    }

    fn delta_requests(&self) -> usize {
        self.graph
            .hits()
            .iter()
            .filter(|h| h.method == "GET" && h.target.contains("/events/delta"))
            .count()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.supervisor.abort();
    }
}

async fn rig(seed: bool) -> Rig {
    let graph = FakeGraph::start(BEARER).await.expect("graph");
    if seed {
        graph.seed_calendars();
    }
    let endpoint = EndpointUrl::parse(graph.base_url()).expect("url");
    let provider = FakeProvider::from_file(PROVIDER);
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: provider.spec().id.clone(),
        label: AccountLabel("ada@outlook.test".into()),
        state: AccountState::Ok,
        auth: AuthKind::OAuthPkce,
        capabilities: provider
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
        endpoints: vec![ServiceEndpoint {
            family: Family::Graph,
            url: endpoint,
            tls: Tls::Plain,
            login: LoginName("ada@outlook.test".into()),
        }],
    };
    let grant = Grant {
        id: GrantId::parse("ms-cal-grant").expect("grant"),
        key: GrantKey {
            app: syncd_app(),
            account: account.id.clone(),
            kind: CapabilityKind::Calendar,
            class: DataClass::Calendar,
            usage: Usage::Background,
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
                account: account.id.clone(),
                purpose: SecretPurpose::OAuthRefresh,
            },
            &Credential::OAuth {
                access: SecretText::new("SECRET-ACCESS-TOKEN"),
                refresh: SecretText::new("SECRET-REFRESH-TOKEN"),
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
    let (network_up, network) = watch::channel(Network::Unmetered);
    let wiring = Wiring {
        accounts: Arc::clone(&accounts),
        hub: hub.clone(),
        paths: paths.clone(),
        settings: Settings::quick(),
        network,
        owners: Access::default(),
    };
    let supervisor = PimSupervisor::new(
        wiring,
        ClientGrants::new(Arc::clone(&accounts)),
        PimConfig {
            rescan: Duration::from_millis(300),
        },
    )
    .spawn();
    Rig {
        _bus: bus,
        graph,
        hub,
        mirrors: paths.mirrors,
        supervisor,
        _network: network_up,
        _keep: vec![accountd, syncd_side],
    }
}

fn timed(uid: &str, subject: &str, hour: u32) -> Value {
    json!({
        "iCalUId": uid, "subject": subject, "type": "singleInstance",
        "start": {"dateTime": format!("2026-10-09T{hour:02}:00:00.0000000"), "timeZone": "UTC"},
        "end": {"dateTime": format!("2026-10-09T{:02}:00:00.0000000", hour + 1), "timeZone": "UTC"},
        "isAllDay": false, "isCancelled": false,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn two_graph_calendars_mirror_into_the_vdir_with_names_colours_and_ics_files() {
    let rig = rig(true).await;
    eventually("every event is mirrored", || {
        rig.items("Personal").len() == 5 && rig.items("Work").len() == 1
    })
    .await;
    let personal = rig.calendar("Personal").expect("personal");
    let work = rig.calendar("Work").expect("work");

    // Names and colours: the hex colour Graph gave, else the named colour's hue.
    assert_eq!(read(&personal.join(COLOR)).as_deref(), Some("#0078d4"));
    assert_eq!(read(&work.join(COLOR)).as_deref(), Some("#6cbf4b"));
    let mut dirs: Vec<String> = std::fs::read_dir(rig.account_dir())
        .expect("account dir")
        .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
        .collect();
    dirs.sort();
    assert!(
        dirs.len() == 2 && dirs.iter().all(|d| d.starts_with("graph-")),
        "{dirs:?}"
    );

    // Every file is a whole iCalendar named by its UID (the exception shares the series' UID, so
    // it is named by the event id's hash).
    let files = items_in(&personal, PimKind::Calendar);
    for file in &files {
        let text = read(&personal.join(file)).expect("file");
        assert!(is_complete(PimKind::Calendar, text.as_bytes()), "{file}");
    }
    for uid in [
        "uid-dentist",
        "uid-standup",
        "uid-holiday",
        "uid-called-off",
    ] {
        assert!(files.contains(&format!("{uid}.ics")), "{uid} in {files:?}");
    }
    let dentist = read(&personal.join("uid-dentist.ics")).expect("dentist");
    assert!(dentist.contains("SUMMARY:Dentist\r\n"), "{dentist}");
    assert!(
        dentist.contains("DTSTART:20261006T100000Z\r\n"),
        "{dentist}"
    );
    assert!(dentist.contains("LOCATION:Main St 1\r\n"), "{dentist}");
    let standup = read(&personal.join("uid-standup.ics")).expect("standup");
    assert!(
        standup.contains("DTSTART;TZID=America/Los_Angeles:20261005T090000\r\n")
            && standup.contains("RRULE:FREQ=WEEKLY;INTERVAL=1;BYDAY=MO;WKST=SU\r\n"),
        "{standup}"
    );
    let moved = rig
        .item_with("Personal", "RECURRENCE-ID:20261012T160000Z")
        .expect("the exception");
    assert!(moved.contains("UID:uid-standup\r\n") && moved.contains("SUMMARY:Standup (moved)"));
    let holiday = read(&personal.join("uid-holiday.ics")).expect("holiday");
    assert!(
        holiday.contains("DTSTART;VALUE=DATE:20261224\r\n"),
        "{holiday}"
    );
    let off = read(&personal.join("uid-called-off.ics")).expect("cancelled");
    assert!(off.contains("STATUS:CANCELLED\r\n"), "{off}");
    let review = rig
        .item_with("Work", "SUMMARY:Design review")
        .expect("review")
        .replace("\r\n ", "");
    assert!(
        review.contains(
            "ATTENDEE;CN=Grace;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:grace@example.test"
        ),
        "{review}"
    );

    // Sync1 names them <account>/<dataset>.
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    let names = rig.hub.names_for(&settings);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names
            .iter()
            .all(|n| n.starts_with(&format!("{SEGMENT}/pim_cal_graph_")))
    );

    // The relay, not syncd, authenticated; nothing was written to the server; no event was
    // fetched singly (a delta page carries the events).
    let hits = rig.graph.hits();
    let calendar_hits: Vec<_> = hits
        .iter()
        .filter(|h| h.target.starts_with("/v1.0/me/calendars"))
        .collect();
    assert!(calendar_hits.len() >= 3);
    let want = format!("Bearer {BEARER}");
    assert!(
        calendar_hits
            .iter()
            .all(|h| h.authorization.as_deref() == Some(want.as_str()))
    );
    assert!(calendar_hits.iter().all(|h| h.method == "GET"));
    assert!(
        calendar_hits
            .iter()
            .all(|h| h.target.contains("/delta") || !h.target.contains("/events/")),
        "{:?}",
        calendar_hits.iter().map(|h| &h.target).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_change_a_removal_a_new_event_and_a_rename_arrive_at_the_next_poll() {
    let rig = rig(true).await;
    eventually("the first mirror", || rig.items("Personal").len() == 5).await;

    rig.graph.put_event(
        "cal-personal",
        "ev-dentist",
        timed("uid-dentist", "Dentist (moved)", 14),
    );
    rig.graph.remove_event("cal-personal", "ev-called-off");
    rig.graph
        .put_event("cal-personal", "ev-new", timed("uid-new", "Gym", 7));
    eventually("change, removal and new event arrive", || {
        rig.item_with("Personal", "SUMMARY:Dentist (moved)")
            .is_some()
            && rig.items("Personal").len() == 5
            && rig.item_with("Personal", "STATUS:CANCELLED").is_none()
            && rig.item_with("Personal", "SUMMARY:Gym").is_some()
    })
    .await;
    let moved = rig
        .item_with("Personal", "SUMMARY:Dentist (moved)")
        .expect("moved");
    assert!(moved.contains("DTSTART:20261009T140000Z\r\n"), "{moved}");

    // A calendar renamed and recoloured, a new one, and a deleted one.
    rig.graph.set_calendar("cal-personal", "Private", "#00ff00");
    rig.graph.set_calendar("cal-new", "Trips", "#123456");
    rig.graph
        .put_event("cal-new", "t1", timed("uid-trip", "Lisbon", 9));
    rig.graph.remove_calendar("cal-work");
    eventually("rename, new calendar and calendar removal arrive", || {
        rig.calendar("Private")
            .is_some_and(|dir| read(&dir.join(COLOR)).as_deref() == Some("#00ff00"))
            && rig.items("Trips").len() == 1
            && rig.calendar("Work").is_none()
            && rig.calendar("Personal").is_none()
    })
    .await;
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    assert_eq!(rig.hub.names_for(&settings).len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_edit_a_deletion_and_a_new_file_are_never_sent_and_a_remote_change_overwrites() {
    let rig = rig(true).await;
    eventually("the first mirror", || rig.items("Personal").len() == 5).await;
    let personal = rig.calendar("Personal").expect("personal");
    std::fs::write(personal.join("uid-dentist.ics"), "locally edited").expect("edit");
    std::fs::remove_file(personal.join("uid-holiday.ics")).expect("delete");
    std::fs::write(
        personal.join("mine.ics"),
        "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
    )
    .expect("new file");
    // Three poll cycles pass.
    tokio::time::sleep(common::poll_time(3500)).await;
    let writes: Vec<_> = rig
        .graph
        .hits()
        .into_iter()
        .filter(|h| h.method != "GET")
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
    // Pull-only: the local state stays until the server next changes those items, which then
    // overwrite it (no conflict); the stray file is left alone.
    assert_eq!(
        read(&personal.join("uid-dentist.ics")).as_deref(),
        Some("locally edited")
    );
    assert!(!personal.join("uid-holiday.ics").exists());
    rig.graph.put_event(
        "cal-personal",
        "ev-dentist",
        timed("uid-dentist", "Dentist again", 8),
    );
    rig.graph.put_event(
        "cal-personal",
        "ev-holiday",
        json!({"iCalUId": "uid-holiday", "subject": "Holiday II", "type": "singleInstance",
               "start": {"dateTime": "2026-12-24T00:00:00.0000000", "timeZone": "UTC"},
               "end": {"dateTime": "2026-12-25T00:00:00.0000000", "timeZone": "UTC"},
               "isAllDay": true, "isCancelled": false}),
    );
    eventually("the server's state replaces the local one", || {
        read(&personal.join("uid-dentist.ics")).is_some_and(|t| t.contains("SUMMARY:Dentist again"))
            && read(&personal.join("uid-holiday.ics")).is_some_and(|t| t.contains("Holiday II"))
    })
    .await;
    assert!(personal.join("mine.ics").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_delta_token_lists_again_and_keeps_every_event() {
    let rig = rig(true).await;
    eventually("the first mirror", || rig.items("Personal").len() == 5).await;
    tokio::time::sleep(common::poll_time(1500)).await;
    let before = rig.delta_requests();
    rig.graph
        .put_event("cal-personal", "ev-extra", timed("uid-extra", "Extra", 6));
    rig.graph.expire_delta_tokens();
    eventually(
        "the new event arrives after the listing starts over",
        || rig.items("Personal").len() == 6,
    )
    .await;
    assert_eq!(rig.items("Work").len(), 1);
    // Work polls on its own schedule: it may not have met its 410 yet when Personal is done.
    eventually("both calendars met the expiry", || {
        rig.graph.hits().iter().filter(|h| h.status == 410).count() >= 2
    })
    .await;
    assert!(rig.delta_requests() > before);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_with_no_calendars_mirrors_nothing_and_says_nothing_is_wrong() {
    let rig = rig(false).await;
    tokio::time::sleep(common::poll_time(1500)).await;
    assert!(
        rig.account_dir()
            .read_dir()
            .map_or(true, |mut d| d.next().is_none())
    );
    assert!(rig.delta_requests() == 0);
}
