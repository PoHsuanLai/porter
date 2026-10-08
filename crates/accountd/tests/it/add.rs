//! `accountd add` as a library call, over the real service, the families of the AI companies, a
//! loopback fake of each company's key check, the file store and audit in scratch directories,
//! in-memory secrets and a scripted terminal. A daemon started afterwards on the same registry
//! sees the account, and the bus name is the lock between the two writers.

use crate::common;

use accountd::add::{AddArgs, AddError, Echo, Terminal, TerminalSheets, run, take_the_name};
use accountd::{FileAudit, FileStore, Options, SecretsDesk, TableCallers, serve_with};
use common::bus::PrivateBus;
use common::{Shared, Tap, contains};
use porter_core::capability::CapabilityKind;
use porter_core::consent::Usage;
use porter_core::store::Persisted;
use porter_core::wire::Refusal;
use porter_core::{AppId, AppName, Credential, DataClass, Isolation, SecretKey, SecretPurpose};
use porter_dbus::{Caller, CallerRole, PeerProxy};
use porter_fake_servers::{Auth, FakeLlmApi, LlmApiHandle, Running, shipped};
use porter_families::{ApiKeyProvider, FamilyProvider};
use porter_http::HyperHttp;
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, RegistryStore};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const KEY: &str = "sk-ant-api03-S3CRET-TYPED-KEY";

/// A terminal that answers from a script and records what it was asked and told.
#[derive(Debug, Default)]
struct Script {
    answers: Mutex<VecDeque<String>>,
    asked: Mutex<Vec<(String, Echo)>>,
    said: Mutex<Vec<String>>,
}

impl Script {
    fn answering(answers: &[&str]) -> Self {
        Self {
            answers: Mutex::new(answers.iter().map(|a| (*a).to_owned()).collect()),
            ..Self::default()
        }
    }

    fn asked(&self) -> Vec<(String, Echo)> {
        self.asked.lock().expect("asked").clone()
    }

    fn said(&self) -> String {
        self.said.lock().expect("said").join("\n")
    }
}

impl Terminal for Script {
    fn say(&self, line: &str) {
        self.said.lock().expect("said").push(line.to_owned());
    }

    fn ask(&self, label: &str, echo: Echo) -> Option<String> {
        self.asked
            .lock()
            .expect("asked")
            .push((label.to_owned(), echo));
        self.answers.lock().expect("answers").pop_front()
    }

    fn open_browser(&self, _url: &str) {}
}

struct Setup {
    dir: PathBuf,
    fake: Running<LlmApiHandle>,
    secrets: Shared,
    store: FileStore,
    audit: PathBuf,
    sheets: TerminalSheets<Script>,
}

impl Setup {
    async fn new(name: &str, answers: &[&str]) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("add-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let fake = FakeLlmApi::start(Auth::XApiKey)
            .await
            .expect("fake company");
        fake.seed_key(KEY);
        Self {
            store: FileStore::new(dir.join("state")),
            audit: dir.join("audit.jsonl"),
            dir,
            fake,
            secrets: Shared::default(),
            sheets: TerminalSheets::new(Script::answering(answers)),
        }
    }

    fn providers(&self) -> Vec<FamilyProvider> {
        let spec = self.fake.point(&shipped::ai("anthropic"));
        vec![FamilyProvider::ApiKey(ApiKeyProvider::with_http(
            spec,
            HyperHttp::default(),
        ))]
    }

    fn served(&self) -> Vec<porter_core::ProviderId> {
        self.providers()
            .iter()
            .map(|p| p.spec().id.clone())
            .collect()
    }

    async fn service(
        &self,
    ) -> AccountService<
        FamilyProvider,
        Shared,
        TerminalSheets<Script>,
        porter_fake::FixedClock,
        FileStore,
        FileAudit,
    > {
        let stored = self.store.load().await.expect("registry loads");
        AccountService::new(
            self.providers(),
            porter_service::Registry::from_persisted(stored),
            self.secrets.clone(),
            self.sheets.clone(),
            porter_fake::FixedClock(porter_fake::NOW),
        )
        .with_store(self.store.clone())
        .with_audit(FileAudit::new(self.audit.clone()))
    }

    async fn stored(&self) -> Persisted {
        self.store.load().await.expect("registry loads")
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn companion() -> AppId {
    AppId {
        name: AppName::parse("org.quire.Companion").expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn args(allow: &[&str], classes: &[&str]) -> AddArgs {
    let own = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    AddArgs::parse("anthropic", &own(allow), &own(classes)).expect("arguments")
}

#[tokio::test]
async fn a_typed_key_becomes_an_account_with_the_apps_grants_and_shows_nowhere() {
    let setup = Setup::new("happy", &[KEY, "y"]).await;
    let service = setup.service().await;
    let report = run(
        &service,
        &setup.sheets,
        &setup.served(),
        &args(&["org.quire.Companion"], &[]),
    )
    .await
    .expect("added");

    // The key was asked for hidden, then the review; and it is on no screen and in no file.
    let terminal = setup.sheets.terminal();
    assert_eq!(
        terminal.asked(),
        [
            ("API key (hidden)".to_owned(), Echo::Off),
            ("Add this account? [Y/n]".to_owned(), Echo::On)
        ]
    );
    assert!(!terminal.said().contains(KEY), "{}", terminal.said());
    let audit = std::fs::read_to_string(&setup.audit).expect("audit file");
    let registry = std::fs::read_to_string(setup.store.path()).expect("registry file");
    for text in [&audit, &registry] {
        assert!(!text.contains(KEY));
    }

    // The account is filed with its key and the Llm claim; the company saw one documented call.
    let stored = setup.stored().await;
    assert_eq!(stored.accounts.len(), 1);
    let account = &stored.accounts[0];
    assert_eq!(account.id, report.account);
    assert_eq!(account.provider.as_str(), "anthropic");
    assert!(
        account
            .capabilities
            .iter()
            .any(|c| c.offer.kind() == CapabilityKind::Llm)
    );
    let key = setup
        .secrets
        .get(&SecretKey {
            account: account.id.clone(),
            purpose: SecretPurpose::ApiKey,
        })
        .await
        .expect("key filed");
    assert_eq!(key, Credential::ApiKey(porter_core::SecretText::new(KEY)));
    let calls = setup.fake.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].x_api_key.as_deref(), Some(KEY));
    assert_eq!(calls[0].anthropic_version.as_deref(), Some("2023-06-01"));

    // One grant per data class, interactive, for the Companion alone, as an Allow-always would.
    assert_eq!(report.grants.len(), accountd::add::ALL_CLASSES.len());
    assert_eq!(stored.grants.len(), report.grants.len());
    for grant in &stored.grants {
        assert_eq!(grant.key.app, companion());
        assert_eq!(grant.key.account, account.id);
        assert_eq!(grant.key.kind, CapabilityKind::Llm);
        assert_eq!(grant.key.usage, Usage::Interactive);
        assert_eq!(grant.scope, porter_core::consent::GrantScope::Always);
    }
    let classes: std::collections::BTreeSet<_> =
        stored.grants.iter().map(|g| g.key.class).collect();
    assert_eq!(classes.len(), accountd::add::ALL_CLASSES.len());
}

#[tokio::test]
async fn classes_narrow_the_grants_and_without_allow_there_are_none() {
    let setup = Setup::new("narrow", &[KEY, "y"]).await;
    let service = setup.service().await;
    let report = run(
        &service,
        &setup.sheets,
        &setup.served(),
        &args(
            &["org.quire.Companion", "org.example.Other:flatpak"],
            &["prompt", "notes", "prompt"],
        ),
    )
    .await
    .expect("added");
    let got: std::collections::BTreeSet<_> = report
        .grants
        .iter()
        .map(|(app, class, _)| (app.name.as_str().to_owned(), app.isolation, *class))
        .collect();
    assert_eq!(report.grants.len(), 4, "{got:?}");
    assert!(got.contains(&(
        "org.example.Other".to_owned(),
        Isolation::Flatpak,
        DataClass::Notes
    )));

    let bare = Setup::new("bare", &[KEY, "y"]).await;
    let service = bare.service().await;
    let report = run(&service, &bare.sheets, &bare.served(), &args(&[], &[]))
        .await
        .expect("added");
    assert!(report.grants.is_empty());
    assert!(bare.stored().await.grants.is_empty());
}

#[tokio::test]
async fn nothing_is_stored_for_a_refused_key_a_declined_review_or_an_unknown_provider() {
    // A key the company refuses.
    let setup = Setup::new("refused", &["sk-wrong"]).await;
    let service = setup.service().await;
    let err = run(&service, &setup.sheets, &setup.served(), &args(&[], &[]))
        .await
        .expect_err("refused");
    assert!(matches!(err, AddError::Refused(_)), "{err:?}");
    assert!(setup.stored().await.accounts.is_empty());

    // A person who says no at the review.
    let setup = Setup::new("declined", &[KEY, "n"]).await;
    let service = setup.service().await;
    let err = run(&service, &setup.sheets, &setup.served(), &args(&[], &[]))
        .await
        .expect_err("declined");
    assert_eq!(err, AddError::Refused(Refusal::Dismissed));
    assert!(setup.stored().await.accounts.is_empty());
    assert!(
        setup
            .secrets
            .get(&SecretKey {
                account: porter_core::AccountId::parse("anthropic").expect("id"),
                purpose: SecretPurpose::ApiKey,
            })
            .await
            .is_err()
    );

    // A provider no file serves: refused before anything is asked.
    let setup = Setup::new("unknown", &[KEY]).await;
    let service = setup.service().await;
    let nobody = AddArgs::parse("nosuch", &[], &[]).expect("arguments");
    let err = run(&service, &setup.sheets, &setup.served(), &nobody)
        .await
        .expect_err("unknown");
    assert!(matches!(err, AddError::UnknownProvider(_)), "{err:?}");
    assert!(setup.sheets.terminal().asked().is_empty());

    // A terminal whose input ended answers nothing, and nothing is added.
    let setup = Setup::new("eof", &[]).await;
    let service = setup.service().await;
    assert!(
        run(&service, &setup.sheets, &setup.served(), &args(&[], &[]))
            .await
            .is_err()
    );
    assert!(setup.stored().await.accounts.is_empty());
}

#[test]
fn the_arguments_are_parsed_as_typed() {
    let own = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let parse = |provider: &str, allow: &[&str], classes: &[&str]| {
        AddArgs::parse(provider, &own(allow), &own(classes))
    };
    let ok = parse("anthropic", &["org.quire.Companion"], &[]).expect("ok");
    assert_eq!(ok.allow, [companion()]);
    let flatpak = parse("openai", &["org.example.App:flatpak"], &["mail"]).expect("ok");
    assert_eq!(flatpak.allow[0].isolation, Isolation::Flatpak);
    assert_eq!(flatpak.classes, [DataClass::Mail]);
    const BAD: &[(&str, &[&str], &[&str])] = &[
        ("Not A Provider", &[], &[]),
        ("openai", &["not a name"], &[]),
        ("openai", &["org.example.App:nowhere"], &[]),
        ("openai", &[], &["secrets"]),
    ];
    for (provider, allow, classes) in BAD {
        assert!(
            matches!(parse(provider, allow, classes), Err(AddError::Argument(_))),
            "{provider} {allow:?} {classes:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bus_name_is_the_lock_between_the_command_and_the_daemon() {
    let bus = PrivateBus::start();
    let (first, second) = (bus.connect().await, bus.connect().await);
    take_the_name(&first).await.expect("free");
    take_the_name(&first).await.expect("already ours");
    assert_eq!(take_the_name(&second).await, Err(AddError::DaemonRunning));
    drop(first);
    let mut taken = false;
    for _ in 0..100 {
        if take_the_name(&second).await.is_ok() {
            taken = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(taken, "released with the connection");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_started_after_the_add_sees_the_account_and_resolves_its_key_and_one_running_blocks_it()
 {
    let setup = Setup::new("daemon", &[KEY, "y"]).await;
    // The add, holding the name the way the command does.
    let bus = PrivateBus::start();
    let locker = bus.connect().await;
    take_the_name(&locker).await.expect("free");
    let service = setup.service().await;
    run(
        &service,
        &setup.sheets,
        &setup.served(),
        &args(&["org.quire.Companion"], &["prompt"]),
    )
    .await
    .expect("added");
    // While it ran, a daemon could not have taken the name.
    let daemon_conn = bus.connect().await;
    let callers = Arc::new(TableCallers::new());
    let started = serve_with(
        &daemon_conn,
        Arc::new(setup.service().await),
        Arc::clone(&callers),
        Options::default(),
    )
    .await;
    assert!(started.is_err(), "the name is held by the command");
    drop(locker);
    drop(daemon_conn);

    // Now the command is over: a daemon reads the registry and serves what was added.
    let daemon_conn = bus.connect().await;
    let desk = SecretsDesk::new(
        setup.secrets.clone(),
        FileAudit::new(setup.audit.clone()),
        porter_fake::FixedClock(porter_fake::NOW),
    );
    let mut started = Err(zbus::Error::Failure("never tried".into()));
    for _ in 0..100 {
        started = serve_with(
            &daemon_conn,
            Arc::new(setup.service().await),
            Arc::clone(&callers),
            Options {
                keys: Some(Arc::new(desk.clone())),
                ..Options::default()
            },
        )
        .await;
        if started.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    started.expect("the daemon serves once the name is free");

    let mut tap = Tap::start(&bus).await;
    let inferd = bus.connect().await;
    callers.introduce_as(
        inferd.unique_name().expect("name").as_str(),
        Caller {
            app: AppId {
                name: AppName::parse("org.quire.Inference").expect("name"),
                isolation: Isolation::Unsandboxed,
            },
            role: CallerRole::PorterDaemon,
        },
    );
    let peer = PeerProxy::new(&inferd).await.expect("proxy");
    let need = porter_dbus::need_to_dbus(&porter_core::Need::Llm(porter_core::need::LlmNeed {
        features: std::collections::BTreeSet::from([porter_core::capability::LlmFeature::Chat]),
        context: porter_core::Tokens(0),
    }));
    let verdicts = peer
        .verdicts(
            &("org.quire.Companion".to_owned(), "unsandboxed".to_owned()),
            &need,
            "prompt",
            "interactive",
        )
        .await
        .expect("verdicts");
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].1, "granted");
    let grant = common::text_of(&verdicts[0].2, "grant").expect("a grant");
    let fd = peer.resolve_key(&grant).await.expect("key");
    let mut text = String::new();
    std::io::Read::read_to_string(
        &mut std::fs::File::from(std::os::fd::OwnedFd::from(fd)),
        &mut text,
    )
    .expect("read");
    assert_eq!(text, KEY);
    // Another class, which the person did not allow, is still asked.
    let other = peer
        .verdicts(
            &("org.quire.Companion".to_owned(), "unsandboxed".to_owned()),
            &need,
            "mail",
            "interactive",
        )
        .await
        .expect("verdicts");
    assert_eq!(other[0].1, "ask");

    // The key crossed no message; the scan can see the grant id, which did.
    let seen = tap.drain().await;
    assert!(seen.iter().all(|m| !contains(m, KEY)));
    assert!(seen.iter().any(|m| contains(m, &grant)), "positive control");
}
