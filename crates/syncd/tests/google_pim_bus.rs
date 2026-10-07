//! A Google account's Calendar, Contacts and Tasks mirrored into the vdir, on one private bus:
//! accountd (the real service over in-memory secrets, a fake provider that mints a token for each
//! endpoint's audience) holds the credential and runs the relay, which adds the bearer; syncd's
//! supervisor, holding a grant per kind whose capability's transport is `google_api`, lists the
//! calendars, the contacts and the task lists of the rig's `FakeGoogle` and mirrors each as
//! `.ics` and `.vcf` files under a scratch `$XDG_DATA_HOME`. Nothing here touches the real
//! session bus, `~/.local` or the network.

mod common;

use accountd::{BusSheets, Options, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::caller as caller_of;
use common::eventually;
use porter_client::{Accounts, DbusTransport};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::{
    Account, AccountId, AccountLabel, AccountState, AppId, AppName, AuthKind, CapabilityKind,
    Claim, Credential, EndpointUrl, Family, GrantId, Isolation, LoginName, Offer, Provenance,
    Restriction, SecretKey, SecretPurpose, SecretText, ServiceEndpoint, SpaceScope, Subject, Tls,
    UnixSeconds,
};
use porter_dbus::{Caller, CallerRole};
use porter_fake::{FakeProvider, FixedClock, MemoryStore, RecordingAudit};
use porter_fake_servers::{FakeGoogle, FakeIssuer, GoogleHandle, IssuerHandle, Running};
use porter_provider::Provider;
use porter_secrets::{MemorySecrets, Secrets};
use porter_service::{AccountService, Registry};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use syncd::datasets::pim::{
    COLOR, ClientGrants, DISPLAYNAME, PimConfig, PimKind, PimSupervisor, Wiring, is_complete,
    items_in,
};
use syncd::paths::Paths;
use syncd::scheduler::{MeteredPolicy, Network, Settings};
use syncd::service::{Access, Hub};
use tokio::sync::watch;

const SYNCD: &str = "org.quire.Sync";
const ACCOUNT: &str = "g-pim";
const SEGMENT: &str = "g_pim";
const PROVIDER: &str = r#"
id = "fake-google"
label = "Fake Google"
mark = "generic"
[auth]
kind = "oauth_pkce"
issuer = "google"
[discovery]
kind = "autoconfig"
[[capability]]
family = "google_calendar"
kind = "calendar"
v = { access = "read_write", delta = "poll", transport = "google_api", collections = "present" }
[[capability]]
family = "google_people"
kind = "contacts"
v = { access = "read_write", delta = "poll", transport = "google_api", collections = "absent" }
[[capability]]
family = "google_tasks"
kind = "tasks"
v = { access = "read_write", delta = "none", transport = "google_api", collections = "present" }
"#;

fn syncd_app() -> AppId {
    AppId {
        name: AppName::parse(SYNCD).expect("app"),
        isolation: Isolation::Flatpak,
    }
}

/// What the fake provider mints for the audience of this account's endpoint of `family`.
fn bearer(family: Family) -> String {
    format!("fake:{ACCOUNT}:{}", family.slug())
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Tells a blocking helper to stop when the test ends, by success or by panic.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct Rig {
    _bus: PrivateBus,
    _issuer: Running<IssuerHandle>,
    google: Running<GoogleHandle>,
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

    /// Every collection directory of the account.
    fn dirs(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.account_dir())
            .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    /// The directory of the collection of `kind` the server calls `name` (directories are named
    /// by a hash of the id, so a test finds one by its `displayname` file).
    fn collection(&self, name: &str, kind: PimKind) -> Option<PathBuf> {
        self.dirs().into_iter().find(|dir| {
            let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned());
            let is = match dir_name.as_deref() {
                Some(n) if n.ends_with("-contacts") => PimKind::Contacts,
                Some(n) if n.ends_with("-tasks") => PimKind::Tasks,
                _ => PimKind::Calendar,
            };
            is == kind && read(&dir.join(DISPLAYNAME)).as_deref() == Some(name)
        })
    }

    fn items(&self, name: &str, kind: PimKind) -> Vec<String> {
        self.collection(name, kind)
            .map(|dir| items_in(&dir, kind))
            .unwrap_or_default()
    }

    /// The text of the item of the collection `name` that holds `needle`.
    fn item_with(&self, name: &str, kind: PimKind, needle: &str) -> Option<String> {
        let dir = self.collection(name, kind)?;
        items_in(&dir, kind)
            .into_iter()
            .filter_map(|file| read(&dir.join(file)))
            .map(|text| text.replace("\r\n ", ""))
            .find(|text| text.contains(needle))
    }

    fn hits_under(&self, prefix: &str) -> Vec<porter_fake_servers::Hit> {
        self.google
            .hits()
            .into_iter()
            .filter(|h| h.target.starts_with(prefix))
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.supervisor.abort();
    }
}

/// A Google account granted to syncd for `kinds`, over a fake Google holding the seeded sets.
async fn rig(kinds: &[PimKind]) -> Rig {
    let issuer = FakeIssuer::start().await.expect("issuer");
    let google = FakeGoogle::start(&issuer).await.expect("google");
    for family in [
        Family::GoogleCalendar,
        Family::GooglePeople,
        Family::GoogleTasks,
    ] {
        google.accept_bearer(&bearer(family));
    }
    google.seed_calendars();
    google.seed_people();
    google.seed_tasks();
    let base = google.base_url().to_owned();
    let endpoint = |family, path: &str| ServiceEndpoint {
        family,
        url: EndpointUrl::parse(&format!("{base}{path}")).expect("url"),
        tls: Tls::Plain,
        login: LoginName("ada@gmail.test".into()),
    };
    let provider = FakeProvider::from_file(PROVIDER);
    let account = Account {
        id: AccountId::parse(ACCOUNT).expect("id"),
        provider: provider.spec().id.clone(),
        label: AccountLabel("ada@gmail.test".into()),
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
        endpoints: vec![
            endpoint(Family::GoogleCalendar, "/calendar/v3"),
            endpoint(Family::GooglePeople, "/v1"),
            endpoint(Family::GoogleTasks, "/tasks/v1"),
        ],
    };
    let grants: Vec<Grant> = kinds
        .iter()
        .map(|kind| Grant {
            id: GrantId::parse(&format!("g-pim-{}", kind.slug())).expect("grant"),
            key: GrantKey {
                app: syncd_app(),
                account: account.id.clone(),
                kind: match kind {
                    PimKind::Calendar => CapabilityKind::Calendar,
                    PimKind::Contacts => CapabilityKind::Contacts,
                    PimKind::Tasks => CapabilityKind::Tasks,
                },
                class: kind.class(),
                usage: Usage::Background,
                space: SpaceScope::Any,
            },
            decision: Decision::Allow,
            scope: GrantScope::Always,
            at: UnixSeconds(1),
        })
        .collect();
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
                grants,
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
        _issuer: issuer,
        google,
        hub,
        mirrors: paths.mirrors,
        supervisor,
        _network: network_up,
        _keep: vec![accountd, syncd_side],
    }
}

/// Whether a path is one item's read (`events.get`, `tasks.get`, `people.get`) rather than a
/// listing.
fn is_single_fetch(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let after = |name: &str| {
        parts
            .iter()
            .position(|p| *p == name)
            .map(|at| &parts[at + 1..])
    };
    after("calendars").is_some_and(|rest| rest.len() == 3)
        || after("lists").is_some_and(|rest| rest.len() == 3)
        || after("people").is_some_and(|rest| rest.len() == 1 && rest[0] != "me")
}

const ALL: [PimKind; 3] = [PimKind::Calendar, PimKind::Contacts, PimKind::Tasks];

fn timed(uid: &str, summary: &str, hour: u32) -> Value {
    json!({
        "iCalUID": uid, "summary": summary,
        "start": {"dateTime": format!("2026-10-09T{hour:02}:00:00Z")},
        "end": {"dateTime": format!("2026-10-09T{:02}:00:00Z", hour + 1)},
    })
}

/// Everything the seeded fake holds is mirrored.
fn everything_mirrored(rig: &Rig) -> bool {
    rig.items("Personal", PimKind::Calendar).len() == 4
        && rig.items("Work", PimKind::Calendar).len() == 1
        && rig.items("Contacts", PimKind::Contacts).len() == 2
        && rig.items("Home", PimKind::Tasks).len() == 2
        && rig.items("Errands", PimKind::Tasks).len() == 1
}

#[tokio::test(flavor = "multi_thread")]
async fn two_calendars_the_contacts_and_two_task_lists_mirror_into_the_vdir() {
    let rig = rig(&ALL).await;
    eventually("every collection is mirrored", || everything_mirrored(&rig)).await;

    // Names and colours; the directories are told apart by kind.
    let personal = rig
        .collection("Personal", PimKind::Calendar)
        .expect("personal");
    let work = rig.collection("Work", PimKind::Calendar).expect("work");
    assert_eq!(read(&personal.join(COLOR)).as_deref(), Some("#9fe1e7"));
    assert_eq!(read(&work.join(COLOR)).as_deref(), Some("#f83a22"));
    let mut dirs: Vec<String> = rig
        .dirs()
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 5, "{dirs:?}");
    assert!(dirs.iter().all(|d| d.starts_with("google-")), "{dirs:?}");
    assert!(dirs.contains(&"google-contacts".to_owned()), "{dirs:?}");
    assert_eq!(dirs.iter().filter(|d| d.ends_with("-tasks")).count(), 2);

    // Calendar: whole iCalendars; the exception shares the series' UID, so it is named by the
    // event id's hash.
    let files = items_in(&personal, PimKind::Calendar);
    for file in &files {
        let text = read(&personal.join(file)).expect("file");
        assert!(is_complete(PimKind::Calendar, text.as_bytes()), "{file}");
    }
    for uid in ["uid-dentist", "uid-standup", "uid-holiday"] {
        assert!(
            files.contains(&format!("{uid}@google.com.ics")),
            "{uid} in {files:?}"
        );
    }
    let dentist = read(&personal.join("uid-dentist@google.com.ics")).expect("dentist");
    for want in [
        "SUMMARY:Dentist\r\n",
        "DTSTART:20261006T170000Z\r\n",
        "LOCATION:Main St 1\r\n",
        "DESCRIPTION:Bring the card\r\n",
    ] {
        assert!(dentist.contains(want), "{want} in {dentist}");
    }
    let standup = read(&personal.join("uid-standup@google.com.ics")).expect("standup");
    assert!(
        standup.contains("DTSTART;TZID=America/Los_Angeles:20261005T090000\r\n")
            && standup.contains("RRULE:FREQ=WEEKLY;BYDAY=MO\r\n"),
        "{standup}"
    );
    let moved = rig
        .item_with("Personal", PimKind::Calendar, "SUMMARY:Standup (moved)")
        .expect("the exception");
    assert!(
        moved.contains("UID:uid-standup@google.com\r\n")
            && moved.contains("RECURRENCE-ID;TZID=America/Los_Angeles:20261012T090000\r\n"),
        "{moved}"
    );
    let holiday = read(&personal.join("uid-holiday@google.com.ics")).expect("holiday");
    assert!(
        holiday.contains("DTSTART;VALUE=DATE:20261224\r\n"),
        "{holiday}"
    );
    let review = rig
        .item_with("Work", PimKind::Calendar, "SUMMARY:Design review")
        .expect("review");
    assert!(
        review.contains(
            "ATTENDEE;CN=Grace;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:grace@example.test"
        ),
        "{review}"
    );

    // Contacts: one collection called Contacts, whole vCards.
    let book = rig.collection("Contacts", PimKind::Contacts).expect("book");
    assert!(read(&book.join(COLOR)).is_none());
    let cards = items_in(&book, PimKind::Contacts);
    assert_eq!(cards, ["people_c1001.vcf", "people_c1002.vcf"]);
    let ada = read(&book.join("people_c1001.vcf")).expect("ada");
    assert!(is_complete(PimKind::Contacts, ada.as_bytes()), "{ada}");
    for want in [
        "VERSION:3.0\r\n",
        "UID:people/c1001\r\n",
        "FN:Ada Lovelace\r\n",
        "EMAIL;TYPE=INTERNET,HOME:ada@example.test\r\n",
        "TEL;TYPE=CELL:+44 20 7946 0000\r\n",
        "ORG:Analytical Engines\r\n",
        "BDAY:1815-12-10\r\n",
    ] {
        assert!(ada.contains(want), "{want} in {ada}");
    }

    // Tasks: whole VTODOs named by id.
    let home = rig.collection("Home", PimKind::Tasks).expect("home");
    assert_eq!(
        items_in(&home, PimKind::Tasks),
        ["t-eggs.ics", "t-milk.ics"]
    );
    let milk = read(&home.join("t-milk.ics")).expect("milk");
    assert!(is_complete(PimKind::Tasks, milk.as_bytes()), "{milk}");
    for want in [
        "BEGIN:VTODO\r\n",
        "UID:t-milk\r\n",
        "SUMMARY:Buy milk\r\n",
        "DESCRIPTION:Semi-skimmed\r\n",
        "DUE;VALUE=DATE:20261012\r\n",
        "STATUS:NEEDS-ACTION\r\n",
    ] {
        assert!(milk.contains(want), "{want} in {milk}");
    }
    let eggs = read(&home.join("t-eggs.ics")).expect("eggs");
    assert!(
        eggs.contains("RELATED-TO;RELTYPE=PARENT:t-milk\r\n"),
        "{eggs}"
    );
    let report = rig
        .item_with("Errands", PimKind::Tasks, "SUMMARY:File the report")
        .expect("report");
    assert!(
        report.contains("STATUS:COMPLETED\r\n")
            && report.contains("COMPLETED:20261007T091500Z\r\n"),
        "{report}"
    );

    // Sync1 names them <account>/<dataset>.
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    let names = rig.hub.names_for(&settings);
    assert_eq!(names.len(), 5, "{names:?}");
    for prefix in [
        "pim_cal_google_",
        "pim_card_google_contacts",
        "pim_task_google_",
    ] {
        assert!(
            names
                .iter()
                .any(|n| n.starts_with(&format!("{SEGMENT}/{prefix}"))),
            "{prefix} in {names:?}"
        );
    }

    // The relay, not syncd, authenticated each API with its own audience's token; nothing was
    // written to the server; no item was fetched singly (the pages carry them).
    let hits = rig.google.hits();
    for (prefix, family) in [
        ("/calendar/v3/", Family::GoogleCalendar),
        ("/v1/people/", Family::GooglePeople),
        ("/tasks/v1/", Family::GoogleTasks),
    ] {
        let under = rig.hits_under(prefix);
        assert!(!under.is_empty(), "{prefix}: {under:?}");
        let want = format!("Bearer {}", bearer(family));
        assert!(
            under
                .iter()
                .all(|h| h.authorization.as_deref() == Some(want.as_str()) && h.status == 200),
            "{prefix}: {under:?}"
        );
    }
    assert!(hits.iter().all(|h| h.method == "GET"));
    assert!(
        hits.iter()
            .all(|h| !is_single_fetch(h.target.split('?').next().unwrap_or(""))),
        "{:?}",
        hits.iter().map(|h| &h.target).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_changes_removals_new_items_and_renames_arrive_at_the_next_poll() {
    let rig = rig(&ALL).await;
    eventually("the first mirror", || everything_mirrored(&rig)).await;

    // Calendar: a changed event, a removed exception, a new event.
    rig.google.put_event(
        "cal-personal",
        "ev-dentist",
        timed("uid-dentist@google.com", "Dentist (moved)", 14),
    );
    rig.google
        .remove_event("cal-personal", "ev-standup_20261012T160000Z");
    rig.google.put_event(
        "cal-personal",
        "ev-new",
        timed("uid-new@google.com", "Gym", 7),
    );
    // Contacts: a changed one, a removed one, a new one.
    rig.google.put_person(
        "people/c1001",
        json!({"names": [{"displayName": "Ada King"}], "emailAddresses": [{"value": "ada@new.test"}]}),
    );
    rig.google.remove_person("people/c1002");
    rig.google
        .put_person("people/c1003", json!({"names": [{"displayName": "Linus"}]}));
    // Tasks: a changed one, a deleted one, a new one, one completed on another device.
    rig.google
        .put_task("list-home", "t-milk", json!({"title": "Buy oat milk"}));
    rig.google.remove_task("list-home", "t-eggs");
    rig.google
        .put_task("list-home", "t-bread", json!({"title": "Bread"}));
    rig.google.put_task(
        "list-work",
        "t-report",
        json!({"title": "File the report", "status": "completed",
               "completed": "2026-10-07T09:15:00.000Z", "notes": "done"}),
    );
    eventually("changes, removals and new items arrive", || {
        rig.item_with("Personal", PimKind::Calendar, "SUMMARY:Dentist (moved)")
            .is_some()
            && rig.items("Personal", PimKind::Calendar).len() == 4
            && rig
                .item_with("Personal", PimKind::Calendar, "Standup (moved)")
                .is_none()
            && rig
                .item_with("Personal", PimKind::Calendar, "SUMMARY:Gym")
                .is_some()
            && rig
                .item_with("Contacts", PimKind::Contacts, "FN:Ada King")
                .is_some()
            && rig.items("Contacts", PimKind::Contacts) == ["people_c1001.vcf", "people_c1003.vcf"]
            && rig
                .item_with("Home", PimKind::Tasks, "SUMMARY:Buy oat milk")
                .is_some()
            && rig.items("Home", PimKind::Tasks) == ["t-bread.ics", "t-milk.ics"]
            && rig
                .item_with("Errands", PimKind::Tasks, "DESCRIPTION:done")
                .is_some()
    })
    .await;
    let moved = rig
        .item_with("Personal", PimKind::Calendar, "SUMMARY:Dentist (moved)")
        .expect("moved");
    assert!(moved.contains("DTSTART:20261009T140000Z\r\n"), "{moved}");

    // A calendar renamed and recoloured, a new one, a deleted one; a task list renamed and one
    // removed.
    rig.google
        .set_calendar("cal-personal", "Private", "#00ff00");
    rig.google.set_calendar("cal-new", "Trips", "#123456");
    rig.google
        .put_event("cal-new", "t1", timed("uid-trip@google.com", "Lisbon", 9));
    rig.google.remove_calendar("cal-work");
    rig.google.set_task_list("list-home", "House");
    rig.google.remove_task_list("list-work");
    eventually("renames, new and removed collections arrive", || {
        rig.collection("Private", PimKind::Calendar)
            .is_some_and(|dir| read(&dir.join(COLOR)).as_deref() == Some("#00ff00"))
            && rig.items("Trips", PimKind::Calendar).len() == 1
            && rig.collection("Work", PimKind::Calendar).is_none()
            && rig.collection("Personal", PimKind::Calendar).is_none()
            && rig.collection("House", PimKind::Tasks).is_some()
            && rig.collection("Errands", PimKind::Tasks).is_none()
    })
    .await;
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    assert_eq!(rig.hub.names_for(&settings).len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_edits_deletions_and_new_files_are_never_sent_and_a_remote_change_overwrites() {
    let rig = rig(&ALL).await;
    eventually("the first mirror", || everything_mirrored(&rig)).await;
    let personal = rig
        .collection("Personal", PimKind::Calendar)
        .expect("personal");
    let book = rig.collection("Contacts", PimKind::Contacts).expect("book");
    let errands = rig.collection("Errands", PimKind::Tasks).expect("errands");
    let before_edit = read(&errands.join("t-report.ics")).expect("report");
    std::fs::write(
        personal.join("uid-dentist@google.com.ics"),
        "locally edited",
    )
    .expect("edit");
    std::fs::remove_file(personal.join("uid-holiday@google.com.ics")).expect("delete");
    std::fs::write(
        personal.join("mine.ics"),
        "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n",
    )
    .expect("new");
    std::fs::write(book.join("people_c1002.vcf"), "locally edited").expect("edit card");
    // The newest task is listed again by every poll (`updatedMin` is inclusive): its unchanged
    // version must not undo a local edit.
    std::fs::write(errands.join("t-report.ics"), "locally edited").expect("edit task");
    // Three poll cycles pass.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let writes: Vec<_> = rig
        .google
        .hits()
        .into_iter()
        .filter(|h| h.method != "GET")
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
    assert_eq!(
        read(&personal.join("uid-dentist@google.com.ics")).as_deref(),
        Some("locally edited")
    );
    assert!(!personal.join("uid-holiday@google.com.ics").exists());
    assert_eq!(
        read(&errands.join("t-report.ics")).as_deref(),
        Some("locally edited")
    );

    // The server's next change to each item replaces the local state (no conflict).
    rig.google.put_event(
        "cal-personal",
        "ev-dentist",
        timed("uid-dentist@google.com", "Dentist again", 8),
    );
    rig.google.put_event(
        "cal-personal",
        "ev-holiday",
        json!({"iCalUID": "uid-holiday@google.com", "summary": "Holiday II",
               "start": {"date": "2026-12-24"}, "end": {"date": "2026-12-25"}}),
    );
    rig.google.put_person(
        "people/c1002",
        json!({"names": [{"displayName": "Grace B. Hopper"}]}),
    );
    rig.google.put_task(
        "list-work",
        "t-report",
        json!({"title": "File the report", "status": "completed",
               "completed": "2026-10-07T09:15:00.000Z"}),
    );
    eventually("the server's state replaces the local one", || {
        read(&personal.join("uid-dentist@google.com.ics"))
            .is_some_and(|t| t.contains("SUMMARY:Dentist again"))
            && read(&personal.join("uid-holiday@google.com.ics"))
                .is_some_and(|t| t.contains("Holiday II"))
            && read(&book.join("people_c1002.vcf")).is_some_and(|t| t.contains("Grace B. Hopper"))
            && read(&errands.join("t-report.ics"))
                .is_some_and(|t| t.contains("SUMMARY:File the report"))
    })
    .await;
    assert!(personal.join("mine.ics").exists());
    assert_ne!(before_edit, "locally edited");
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_sync_tokens_list_again_and_keep_every_item() {
    let rig = rig(&ALL).await;
    eventually("the first mirror", || everything_mirrored(&rig)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let before = rig.google.hits().len();
    rig.google.put_event(
        "cal-personal",
        "ev-extra",
        timed("uid-extra@google.com", "Extra", 6),
    );
    rig.google.put_person(
        "people/c1004",
        json!({"names": [{"displayName": "Extra Person"}]}),
    );
    rig.google.expire_sync_tokens();
    eventually("the new items arrive after the listings start over", || {
        rig.items("Personal", PimKind::Calendar).len() == 5
            && rig.items("Contacts", PimKind::Contacts).len() == 3
    })
    .await;
    // Nothing was lost, and the sources met the two refusals: 410 for each calendar, 400 for
    // the contacts.
    assert!(everything_mirrored_after_extras(&rig));
    let after = rig.google.hits().split_off(before);
    let events_gone = after
        .iter()
        .filter(|h| h.status == 410 && h.target.starts_with("/calendar/v3/calendars/"))
        .count();
    let contacts_expired = after
        .iter()
        .filter(|h| h.status == 400 && h.target.starts_with("/v1/people/me/connections"))
        .count();
    assert!(
        events_gone >= 2,
        "both calendars met the expiry: {events_gone}"
    );
    assert!(contacts_expired >= 1, "{contacts_expired}");
}

fn everything_mirrored_after_extras(rig: &Rig) -> bool {
    rig.items("Work", PimKind::Calendar).len() == 1
        && rig.items("Home", PimKind::Tasks).len() == 2
        && rig.items("Errands", PimKind::Tasks).len() == 1
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_never_sees_a_file_that_is_not_a_complete_item() {
    let rig = rig(&ALL).await;
    eventually("the first mirror", || everything_mirrored(&rig)).await;
    let personal = rig
        .collection("Personal", PimKind::Calendar)
        .expect("personal");
    let big = |round: u32, n: u32| {
        let mut event = timed(&format!("big{n}@google.com"), &format!("round-{round}"), 5);
        event["description"] =
            json!("0123456789abcdef0123456789abcdef0123456789abcdef\n".repeat(600));
        event
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
                std::thread::sleep(Duration::from_millis(2));
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
            rig.google
                .put_event("cal-personal", &format!("big{n}"), big(round, n));
        }
        let marker = format!("SUMMARY:round-{round}");
        eventually("the round is mirrored", || {
            (0..20).all(|n| {
                read(&personal.join(format!("big{n}@google.com.ics")))
                    .is_some_and(|t| t.contains(&marker))
            })
        })
        .await;
    }
    stop.store(true, Ordering::Relaxed);
    let (reads, torn) = reader.await.expect("reader");
    assert!(
        reads > 50,
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
async fn a_tasks_grant_without_a_calendar_grant_mirrors_only_tasks() {
    let rig = rig(&[PimKind::Tasks]).await;
    eventually("the task lists are mirrored", || {
        rig.items("Home", PimKind::Tasks).len() == 2
            && rig.items("Errands", PimKind::Tasks).len() == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(rig.dirs().len(), 2, "{:?}", rig.dirs());
    assert!(
        rig.dirs()
            .iter()
            .all(|d| d.to_string_lossy().ends_with("-tasks")),
        "{:?}",
        rig.dirs()
    );
    assert!(rig.hits_under("/calendar/v3/").is_empty());
    assert!(rig.hits_under("/v1/people/").is_empty());
    assert!(!rig.hits_under("/tasks/v1/").is_empty());
    let settings = caller_of("org.quire.Settings", CallerRole::Settings);
    assert_eq!(rig.hub.names_for(&settings).len(), 2);
}
