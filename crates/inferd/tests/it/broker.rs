//! inferd's two seams over accountd's `Broker`, against a hand-written Broker on a private bus
//! that knows inferd as the AI broker: the permits an app holds as verdicts, a key read from the
//! descriptor it arrives on, the servers of the person's own, a probed runtime reported as an
//! account, and what a refusal or a missing accountd is to the one who asked. The keys are fakes.

use crate::hosting::bus::{PrivateBus, within};

use inferd::broker::{AccountList, AccountdBroker, follow};
use inferd::cloud::accountd::{Accountd, AccountdFault};
use inferd::probe::Runtime;
use inferd::probed::Standing;
use inferd::report::{ReportFault, Reports};
use porter_client::AiAccounts;
use porter_core::consent::{GrantScope, Usage, Verdict};
use porter_core::{
    AccountId, AccountState, AiAccountRow, AppId, AppName, DataClass, EndpointUrl, Family, GrantId,
    Isolation, LlmVerdict, LoginName, OwnServerRow, ServiceEndpoint, Tls,
};
use porter_dbus::Call;
use porter_dbus::accounts::{BrokerServed, BrokerServer, RowArg};
use porter_dbus::accounts::{ai_account_to_dbus, own_server_to_dbus, verdict_to_dbus};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use zbus::zvariant::OwnedFd;

const KEY: &str = "sk-fake-inferd-0001";

fn notes() -> AppId {
    AppId {
        name: AppName::parse("org.example.Notes").expect("name"),
        isolation: Isolation::Flatpak,
    }
}

fn id(text: &str) -> AccountId {
    AccountId::parse(text).expect("id")
}

/// What the fake Broker was asked and what it answers.
#[derive(Debug, Default)]
struct Script {
    reported: Vec<(String, String, String)>,
    verdict_calls: Vec<(String, String, String, String)>,
}

#[derive(Clone)]
struct FakeBroker {
    script: Arc<Mutex<Script>>,
}

/// An error reply as accountd answers one: `org.quire.Accounts1.Error.<Refusal>`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.quire.Accounts1.Error")]
enum Fault {
    #[zbus(error)]
    ZBus(zbus::Error),
    UnknownGrant(String),
    Denied(String),
    NeedsReauth(String),
    Unavailable(String),
}

impl Fault {
    fn of(name: &str) -> Self {
        match name {
            "Denied" => Fault::Denied(String::new()),
            "NeedsReauth" => Fault::NeedsReauth(String::new()),
            "Unavailable" => Fault::Unavailable(String::new()),
            _ => Fault::UnknownGrant(String::new()),
        }
    }
}

impl BrokerServer for FakeBroker {
    type Error = Fault;

    async fn verdicts(
        &self,
        Call { .. }: Call<'_>,
        app: (String, String),
        class: String,
        usage: String,
    ) -> Result<Vec<RowArg>, Fault> {
        self.script
            .lock()
            .expect("lock")
            .verdict_calls
            .push((app.0, app.1, class, usage));
        let granted = LlmVerdict::new(
            id("openrouter"),
            "openrouter".into(),
            "Work".into(),
            Some("OpenRouter".into()),
            AccountState::Ok,
            Verdict::Granted {
                grant: GrantId::parse("g.1").expect("id"),
                scope: GrantScope::Always,
            },
        );
        let asked = LlmVerdict::new(
            id("anthropic"),
            "anthropic".into(),
            "Anthropic".into(),
            None,
            AccountState::NeedsReauth,
            Verdict::Ask,
        );
        Ok(vec![verdict_to_dbus(&granted), verdict_to_dbus(&asked)])
    }

    async fn read_key(&self, Call { .. }: Call<'_>, grant: String) -> Result<OwnedFd, Fault> {
        match grant.as_str() {
            "g.1" => porter_secrets::sealed_key(KEY)
                .map(OwnedFd::from)
                .map_err(|_| Fault::of("Unavailable")),
            "g-off" => Err(Fault::of("Denied")),
            "g-sign-in" => Err(Fault::of("NeedsReauth")),
            _ => Err(Fault::of("UnknownGrant")),
        }
    }

    async fn accounts(&self, Call { .. }: Call<'_>) -> Result<Vec<RowArg>, Fault> {
        Ok(vec![
            ai_account_to_dbus(&AiAccountRow::new(
                id("openrouter"),
                "openrouter".into(),
                Some("OpenRouter".into()),
                "Work".into(),
                true,
            )),
            ai_account_to_dbus(&AiAccountRow::new(
                id("ollama"),
                "ollama".into(),
                None,
                "Ollama".into(),
                false,
            )),
        ])
    }

    async fn servers(&self, Call { .. }: Call<'_>) -> Result<Vec<RowArg>, Fault> {
        let endpoint = ServiceEndpoint {
            family: Family::ChatCompletions,
            url: EndpointUrl::parse("http://127.0.0.1:8080/v1").expect("url"),
            tls: Tls::Plain,
            login: LoginName(String::new()),
        };
        Ok(vec![own_server_to_dbus(&OwnServerRow::new(
            id("mine"),
            "Mine".into(),
            endpoint,
        ))])
    }

    async fn read_server_key(
        &self,
        Call { .. }: Call<'_>,
        account: String,
    ) -> Result<OwnedFd, Fault> {
        match account.as_str() {
            "mine" => porter_secrets::sealed_key(KEY)
                .map(OwnedFd::from)
                .map_err(|_| Fault::of("Unavailable")),
            _ => Err(Fault::of("UnknownGrant")),
        }
    }

    async fn report_local(
        &self,
        Call { .. }: Call<'_>,
        provider: String,
        claims: String,
        state: String,
    ) -> Result<String, Fault> {
        self.script
            .lock()
            .expect("lock")
            .reported
            .push((provider.clone(), claims, state));
        Ok(provider)
    }
}

struct Rig {
    bus: PrivateBus,
    _daemon: zbus::Connection,
    script: Arc<Mutex<Script>>,
    broker: AccountdBroker,
    accounts: AiAccounts,
}

async fn rig() -> Rig {
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    let script = Arc::new(Mutex::new(Script::default()));
    daemon
        .object_server()
        .at(
            porter_wire::names::ACCOUNTS_PATH,
            BrokerServed(FakeBroker {
                script: Arc::clone(&script),
            }),
        )
        .await
        .expect("serve");
    daemon
        .request_name(porter_wire::names::ACCOUNTS_BUS)
        .await
        .expect("name");
    let accounts = AiAccounts::connect(&bus.connect().await)
        .await
        .expect("connect");
    Rig {
        bus,
        _daemon: daemon,
        script,
        broker: AccountdBroker::new(accounts.clone()),
        accounts,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_verdicts_name_the_account_its_provider_its_state_and_what_the_permits_say() {
    let rig = rig().await;
    let rows = within(
        "the verdicts",
        rig.broker
            .verdicts(&notes(), DataClass::Prompt, Usage::Interactive),
    )
    .await
    .expect("answered");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].account, id("openrouter"));
    assert_eq!(rows[0].provider_label.as_deref(), Some("OpenRouter"));
    assert_eq!(rows[0].state, Some(AccountState::Ok));
    assert!(matches!(rows[0].verdict, Verdict::Granted { .. }));
    assert_eq!(rows[1].verdict, Verdict::Ask);
    assert_eq!(rows[1].state, Some(AccountState::NeedsReauth));
    assert_eq!(
        rig.script.lock().expect("lock").verdict_calls,
        [(
            "org.example.Notes".to_owned(),
            "flatpak".to_owned(),
            "prompt".to_owned(),
            "interactive".to_owned()
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_arrives_on_a_descriptor_and_a_refusal_is_a_fault_not_a_key() {
    let rig = rig().await;
    let grant = |text: &str| GrantId::parse(text).expect("id");
    let key = rig.broker.key(&grant("g.1")).await.expect("the key");
    assert_eq!(key.expose(), KEY);
    assert!(!format!("{key:?}").contains("sk-fake"));
    // Every refusal is final; none carries a key.
    for permit in ["g-off", "g-sign-in", "g-none"] {
        assert_eq!(
            rig.broker.key(&grant(permit)).await.map(drop),
            Err(AccountdFault::Refused),
            "{permit}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_servers_of_the_persons_own_and_their_keys() {
    let rig = rig().await;
    let servers = rig.broker.servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].account, id("mine"));
    assert_eq!(servers[0].label, "Mine");
    let key = rig.broker.server_key(&id("mine")).await.expect("key");
    assert_eq!(key.expose(), KEY);
    assert_eq!(
        rig.broker.server_key(&id("other")).await.map(drop),
        Err(AccountdFault::Refused)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probed_runtime_is_reported_with_its_provider_and_state() {
    let rig = rig().await;
    let reported = rig
        .broker
        .report(Runtime::Ollama, &[], Standing::Online)
        .await
        .expect("reported");
    assert_eq!(reported.as_str(), "ollama");
    rig.broker
        .report(Runtime::Ollama, &[], Standing::Offline)
        .await
        .expect("reported");
    let seen = rig.script.lock().expect("lock").reported.clone();
    assert_eq!(
        seen.iter()
            .map(|(provider, _, state)| (provider.as_str(), state.as_str()))
            .collect::<Vec<_>>(),
        [("ollama", "ok"), ("ollama", "offline")]
    );
    assert!(seen.iter().all(|(_, claims, _)| claims == "[]"));
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_accountd_on_the_bus_a_question_is_ask_again_and_a_report_is_unreachable() {
    let bus = PrivateBus::start();
    let accounts = AiAccounts::connect(&bus.connect().await)
        .await
        .expect("connect");
    let broker = AccountdBroker::new(accounts);
    assert_eq!(
        broker
            .verdicts(&notes(), DataClass::Prompt, Usage::Interactive)
            .await
            .map(drop),
        Err(AccountdFault::Unreachable)
    );
    assert!(broker.servers().await.is_empty());
    assert_eq!(
        broker
            .report(Runtime::Ollama, &[], Standing::Online)
            .await
            .map(drop),
        Err(ReportFault::Unreachable)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_of_accounts_is_kept_and_the_news_is_woken_once_the_first_look_is_made() {
    let rig = rig().await;
    let list = AccountList::default();
    let news = Arc::new(Notify::new());
    let (first, told) = tokio::sync::oneshot::channel();
    follow(
        rig.accounts.clone(),
        list.clone(),
        Arc::clone(&news),
        move || {
            let _ = first.send(());
        },
    );
    within("the first look", told).await.expect("told");
    within("the news", news.notified()).await;
    let now = list.now();
    assert_eq!(now.len(), 2);
    assert!(
        now.iter()
            .any(|a| a.account == id("openrouter") && a.network)
    );
    assert!(now.iter().any(|a| a.account == id("ollama") && !a.network));
    drop(rig.bus);
}
