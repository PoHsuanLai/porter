//! Accounts for external coding agents that sign themselves in (agent-session ask P1): adding
//! one asks nothing secret and stores no credential, and only the agent launcher may say what
//! the agent reports (`Peer.SetAgentState`), and it may say nothing else.

use crate::common;

use accountd::add::{AddArgs, Echo, Terminal, TerminalSheets, run};
use accountd::{FileAudit, FileStore};
use common::agents::*;
use common::*;
use ds_settings::live::LiveError;
use porter_core::capability::{AgentProgram, Offered};
use porter_core::need::AgentNeed;
use porter_core::store::Persisted;
use porter_core::{
    Account, AccountId, AccountState, AuthKind, CapabilityKind, Match, Need, ProviderId, SecretKey,
    SecretPurpose, Shortfall, Subject, matches,
};
use porter_dbus::{CallerRole, ManagerProxy, PeerProxy, TokensProxy};
use porter_families::{AgentLoginProvider, FamilyProvider};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, RegistryStore};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn claude_code() -> AccountId {
    AccountId::parse("claude-code").expect("id")
}

fn state_of(rig: &Rig, id: &AccountId) -> AccountState {
    rig.service
        .registry()
        .accounts
        .iter()
        .find(|a| a.id == *id)
        .map(|a| a.state)
        .expect("the account")
}

const LAUNCHER: &str = "org.example.Launcher";

async fn peer_as(rig: &Rig, who: CallerRole) -> PeerProxy<'static> {
    let connection = rig.client_as(caller(LAUNCHER, who)).await;
    PeerProxy::new(&connection).await.expect("proxy")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_launcher_reports_an_agents_state_and_the_account_follows_it() {
    let rig = rig_with_an_agent(AccountState::NeedsLogin).await;
    let peer = peer_as(&rig, CallerRole::AgentLauncher).await;

    peer.set_agent_state("claude-code", "ready")
        .await
        .expect("accepted");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
    // The other agent's account is its own.
    assert_eq!(
        state_of(&rig, &AccountId::parse("codex").expect("id")),
        AccountState::NeedsLogin
    );
    // Saying it again changes nothing and is not an error.
    peer.set_agent_state("claude-code", "ready")
        .await
        .expect("accepted again");
    peer.set_agent_state("claude-code", "needs_login")
        .await
        .expect("accepted");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    // The change was saved with the registry.
    let saved = rig.store.stored().expect("saved").accounts;
    let saved = saved
        .iter()
        .find(|a| a.id == claude_code())
        .expect("account");
    assert_eq!(saved.state, AccountState::NeedsLogin);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_other_role_and_a_stranger_is_refused_set_agent_state() {
    let rig = rig_with_an_agent(AccountState::NeedsLogin).await;
    for who in [
        CallerRole::App,
        CallerRole::Settings,
        CallerRole::SheetHost,
        CallerRole::PorterDaemon,
        CallerRole::Agent,
        CallerRole::Cua,
    ] {
        let peer = peer_as(&rig, who).await;
        let refused = peer
            .set_agent_state("claude-code", "ready")
            .await
            .expect_err("refused");
        assert_eq!(error_name(&refused), ACCESS_DENIED, "{who:?}");
    }
    let stranger = rig.stranger().await;
    let peer = PeerProxy::new(&stranger).await.expect("proxy");
    let refused = peer
        .set_agent_state("claude-code", "ready")
        .await
        .expect_err("refused");
    assert_eq!(error_name(&refused), ACCESS_DENIED);
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_launcher_may_say_nothing_else() {
    let rig = rig_with_an_agent(AccountState::Ok).await;
    let connection = rig
        .client_as(caller(LAUNCHER, CallerRole::AgentLauncher))
        .await;
    let peer = PeerProxy::new(&connection).await.expect("proxy");
    let manager = ManagerProxy::new(&connection).await.expect("proxy");
    let tokens = TokensProxy::new(&connection).await.expect("proxy");
    let denied = |error: zbus::Error| error_name(&error);

    assert_eq!(
        denied(
            manager
                .query(&storage_need(), "photos", "interactive")
                .await
                .expect_err("query")
        ),
        ACCESS_DENIED
    );
    assert_eq!(
        denied(
            manager
                .add_account("", "", &porter_dbus::Details::new())
                .await
                .expect_err("add")
        ),
        ACCESS_DENIED
    );
    assert_eq!(
        denied(tokens.issue_token("g1", "webdav").await.expect_err("token")),
        ACCESS_DENIED
    );
    assert_eq!(
        denied(peer.resolve_key("g1").await.expect_err("key")),
        ACCESS_DENIED
    );
    assert_eq!(
        denied(
            peer.report_local("ollama", Vec::new(), "ok")
                .await
                .expect_err("report")
        ),
        ACCESS_DENIED
    );
    assert!(rig.host_log.calls().opened.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_for_no_agent_account_or_in_no_known_word_is_invalid() {
    let rig = rig_with_an_agent(AccountState::NeedsLogin).await;
    let peer = peer_as(&rig, CallerRole::AgentLauncher).await;
    const INVALID: &str = "org.freedesktop.DBus.Error.InvalidArgs";
    for (what, account, state) in [
        ("an unknown account", "nosuch", "ready"),
        ("an account that is not an agent", "fake-storage", "ready"),
        ("a word that is not a state", "claude-code", "signed-in"),
        (
            "a state of the account's own",
            "claude-code",
            "needs_reauth",
        ),
        ("an id that is no id", "Not An Id", "ready"),
    ] {
        let refused = peer
            .set_agent_state(account, state)
            .await
            .expect_err("refused");
        assert_eq!(error_name(&refused), INVALID, "{what}");
    }
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    assert_eq!(
        state_of(&rig, &storage_account().id),
        AccountState::Ok,
        "an account that is not an agent keeps its state"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_state_shows_in_the_settings_schema_and_signing_out_forgets_it() {
    use zbus::export::futures_core::Stream;
    let rig = rig_with_an_agent(AccountState::NeedsLogin).await;
    let client = settings(&rig).await;
    let schema = client.describe().await.expect("schema");
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    for want in [
        "accounts.claude-code.label",
        "accounts.claude-code.state",
        "accounts.claude-code.service.agent",
        "accounts.claude-code.reauth",
        "accounts.claude-code.remove",
    ] {
        assert!(paths.contains(&want), "missing {want} in {paths:?}");
    }
    // Not signed in: "Sign in" (which asks the agent's launcher), not "Sign out" (ux-9).
    assert!(
        !paths.contains(&"accounts.claude-code.sign_out"),
        "{paths:?}"
    );
    assert!(paths.contains(&"accounts.fake-storage.reauth"), "{paths:?}");
    assert_eq!(
        client
            .get(&key("accounts.claude-code.state"))
            .await
            .expect("state"),
        toml::Value::String("needs_login".into())
    );

    // The launcher hears the agent say it is signed in: the row follows, and Settings is told.
    let mut changes = client.changes().await.expect("changes");
    let peer = peer_as(&rig, CallerRole::AgentLauncher).await;
    peer.set_agent_state("claude-code", "ready")
        .await
        .expect("accepted");
    let change = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)),
    )
    .await
    .expect("a Changed in time")
    .expect("open stream")
    .expect("a change");
    assert_eq!(change.key, key("accounts.claude-code.state"));
    assert_eq!(change.value, toml::Value::String("ok".into()));
    assert_eq!(
        client
            .get(&key("accounts.claude-code.state"))
            .await
            .expect("state"),
        toml::Value::String("ok".into())
    );
    // Signed in: the schema now offers "Sign out" and not "Sign in".
    let schema = client.describe().await.expect("schema");
    let paths: Vec<&str> = schema.key.iter().map(|k| k.path.0.as_str()).collect();
    assert!(
        paths.contains(&"accounts.claude-code.sign_out"),
        "{paths:?}"
    );
    assert!(!paths.contains(&"accounts.claude-code.reauth"), "{paths:?}");

    // Sign out forgets that, and only that: no secret is touched, the account stays.
    client
        .invoke(&key("accounts.claude-code.sign_out"))
        .await
        .expect("signed out");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    assert_eq!(rig.service.registry().accounts.len(), 3);
    // Only Settings may.
    let app = settings_as(&rig, caller("org.example.App", CallerRole::App)).await;
    assert!(matches!(
        app.invoke(&key("accounts.claude-code.sign_out")).await,
        Err(LiveError::NotPermitted(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_going_to_needs_login_is_told_to_the_shell_as_a_refused_account_is() {
    let rig = rig_with_an_agent(AccountState::Ok).await;
    // The shell asks which accounts need a person, which joins it to the roster.
    let shell = rig
        .client_as(caller("org.example.Shell", CallerRole::SheetHost))
        .await;
    let manager = ManagerProxy::new(&shell).await.expect("proxy");
    assert!(manager.needing_reauth().await.expect("list").is_empty());
    let mut told = listen(&shell).await;

    let peer = peer_as(&rig, CallerRole::AgentLauncher).await;
    peer.set_agent_state("claude-code", "needs_login")
        .await
        .expect("accepted");
    let heard_by_shell = heard(&mut told, std::time::Duration::from_secs(2)).await;
    assert!(
        heard_by_shell.contains(&"NeedsReauth".to_owned()),
        "{heard_by_shell:?}"
    );
    let needing = manager.needing_reauth().await.expect("list");
    assert_eq!(needing.len(), 1);
    assert_eq!(needing[0].1, "Claude Code");
}

// The add flow, over the real AgentLogin family and the shipped provider file.

#[derive(Debug, Default)]
struct Script {
    answers: Mutex<VecDeque<String>>,
    asked: Mutex<Vec<(String, Echo)>>,
}

impl Terminal for Script {
    fn say(&self, _line: &str) {}

    fn ask(&self, label: &str, echo: Echo) -> Option<String> {
        self.asked
            .lock()
            .expect("asked")
            .push((label.to_owned(), echo));
        self.answers.lock().expect("answers").pop_front()
    }

    fn open_browser(&self, _url: &str) {}
}

/// A secret store that remembers every write, over the shared in-memory one.
#[derive(Debug, Clone, Default)]
struct Tally {
    inner: Shared,
    writes: Arc<Mutex<Vec<SecretKey>>>,
}

impl Secrets for Tally {
    async fn put(
        &self,
        key: &SecretKey,
        value: &porter_core::Credential,
    ) -> Result<(), porter_secrets::SecretsError> {
        self.writes.lock().expect("writes").push(key.clone());
        self.inner.put(key, value).await
    }
    async fn get(
        &self,
        key: &SecretKey,
    ) -> Result<porter_core::Credential, porter_secrets::SecretsError> {
        self.inner.get(key).await
    }
    async fn delete(&self, key: &SecretKey) -> Result<(), porter_secrets::SecretsError> {
        self.inner.delete(key).await
    }
    async fn delete_account(
        &self,
        account: &AccountId,
    ) -> Result<(), porter_secrets::SecretsError> {
        self.inner.delete_account(account).await
    }
}

#[tokio::test]
async fn adding_an_agent_account_asks_nothing_secret_and_stores_no_credential() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("agent-add-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let store = FileStore::new(dir.join("state"));
    let audit = dir.join("audit.jsonl");
    let secrets = Tally::default();
    let script = Script {
        answers: Mutex::new(VecDeque::from(["y".to_owned()])),
        ..Script::default()
    };
    let sheets = TerminalSheets::new(script);
    let providers = vec![FamilyProvider::AgentLogin(AgentLoginProvider::new(
        shipped("claude-code"),
    ))];
    let served: Vec<ProviderId> = providers.iter().map(|p| p.spec().id.clone()).collect();
    let service = AccountService::new(
        providers,
        porter_service::Registry::from_persisted(store.load().await.expect("loads")),
        secrets.clone(),
        sheets.clone(),
        porter_fake::FixedClock(porter_fake::NOW),
    )
    .with_store(store.clone())
    .with_audit(FileAudit::new(audit.clone()));

    let args = AddArgs::parse("claude-code", &[], &[]).expect("arguments");
    let report = run(&service, &sheets, &served, &args).await.expect("added");

    // Nothing hidden was asked for: the only question is the review.
    let asked = sheets.terminal().asked.lock().expect("asked").clone();
    assert_eq!(asked, [("Add this account? [Y/n]".to_owned(), Echo::On)]);
    assert!(asked.iter().all(|(_, echo)| *echo == Echo::On));

    // The account is there, waiting for the agent to say it is signed in.
    let stored: Persisted = store.load().await.expect("loads");
    assert_eq!(stored.accounts.len(), 1);
    let account = &stored.accounts[0];
    assert_eq!(account.id, report.account);
    assert_eq!(account.provider.as_str(), "claude-code");
    assert_eq!(account.auth, AuthKind::AgentLogin);
    assert_eq!(account.state, AccountState::NeedsLogin);
    assert!(account.endpoints.is_empty());
    let programs: Vec<_> = account
        .capabilities
        .iter()
        .filter_map(|claim| match &claim.subject {
            Subject::Agent(program) => Some(program.as_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(programs, ["claude-code"]);

    // No credential of any kind: not under any purpose in the secret store, and no secret
    // vocabulary in the registry or the audit file.
    for purpose in [
        SecretPurpose::Password,
        SecretPurpose::IncomingPassword,
        SecretPurpose::OutgoingPassword,
        SecretPurpose::OAuthRefresh,
        SecretPurpose::ApiKey,
        SecretPurpose::KeyPair,
    ] {
        let key = SecretKey {
            account: account.id.clone(),
            purpose,
        };
        assert!(secrets.get(&key).await.is_err(), "{purpose:?}");
    }
    assert!(
        secrets.writes.lock().expect("writes").is_empty(),
        "the secret store was never written"
    );
    let registry = std::fs::read_to_string(store.path()).expect("registry file");
    for word in ["secret", "password", "refresh", "sk-"] {
        assert!(
            !registry.to_lowercase().contains(word),
            "{word} in registry"
        );
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn allow_on_an_agent_account_adds_it_and_makes_no_grant() {
    let secrets = Tally::default();
    let sheets = TerminalSheets::new(Script {
        answers: Mutex::new(VecDeque::from(["y".to_owned()])),
        ..Script::default()
    });
    let providers = vec![FamilyProvider::AgentLogin(AgentLoginProvider::new(
        shipped("codex"),
    ))];
    let served: Vec<ProviderId> = providers.iter().map(|p| p.spec().id.clone()).collect();
    let service = AccountService::new(
        providers,
        porter_service::Registry::default(),
        secrets.clone(),
        sheets.clone(),
        porter_fake::FixedClock(porter_fake::NOW),
    );
    let args = AddArgs::parse("codex", &["org.quire.Companion".to_owned()], &[]).expect("args");
    let err = run(&service, &sheets, &served, &args)
        .await
        .expect_err("no grant for an agent");
    let accountd::add::AddError::AllowFailed { account, refusal } = err else {
        panic!("{err:?}");
    };
    assert_eq!(refusal, porter_core::wire::Refusal::NoFittingAccount);
    let registry = service.registry();
    assert_eq!(registry.accounts.len(), 1);
    assert_eq!(registry.accounts[0].id, account);
    assert!(registry.grants.is_empty());
    assert!(secrets.writes.lock().expect("writes").is_empty());
}

// Which account runs which agent.

fn need(program: &str, base_url: Offered) -> Need {
    Need::Agent(AgentNeed {
        program: AgentProgram::parse(program).expect("program"),
        protocols: Default::default(),
        base_url,
    })
}

/// How an account's claims answer a need: the first claim of the kind that fits, else the first
/// shortfall.
fn answer(account: &Account, need: &Need) -> Match {
    let answers: Vec<Match> = account
        .capabilities
        .iter()
        .filter(|claim| claim.offer.kind() == CapabilityKind::Agent)
        .map(|claim| matches(need, &claim.offer))
        .collect();
    answers
        .iter()
        .find(|m| **m == Match::Fits)
        .or(answers.first())
        .copied()
        .unwrap_or(Match::OtherKind)
}

#[test]
fn each_program_is_run_by_its_own_account_and_a_key_account_only_by_the_programs_its_file_names() {
    // A key account holds the agent claims its provider file declares, as the ApiKey family
    // files them; here those are read off the shipped files the same way.
    let key_account = |provider: &str| Account {
        auth: AuthKind::ApiKey,
        ..agent_account(provider, AccountState::Ok)
    };
    let accounts: Vec<(&str, Account)> = vec![
        (
            "claude-code login",
            agent_account("claude-code", AccountState::Ok),
        ),
        (
            "gemini-cli login",
            agent_account("gemini-cli", AccountState::Ok),
        ),
        ("codex login", agent_account("codex", AccountState::Ok)),
        (
            "acp-agent login",
            agent_account("acp-agent", AccountState::Ok),
        ),
        ("anthropic key", key_account("anthropic")),
        ("openai key", key_account("openai")),
        ("google-ai key", key_account("google-ai")),
        ("openrouter key", key_account("openrouter")),
    ];
    // (program, the accounts that run it)
    const WANT: &[(&str, &[&str])] = &[
        ("claude-code", &["claude-code login", "anthropic key"]),
        ("gemini-cli", &["gemini-cli login", "google-ai key"]),
        ("codex", &["codex login", "openai key"]),
        ("acp-agent", &["acp-agent login"]),
        ("aider", &[]),
    ];
    for (program, expected) in WANT {
        let running: Vec<&str> = accounts
            .iter()
            .filter(|(_, account)| answer(account, &need(program, Offered::Absent)) == Match::Fits)
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(&running, expected, "{program}");
    }
    // A key account is refused for the program of another company, and says why.
    let anthropic = &accounts
        .iter()
        .find(|(n, _)| *n == "anthropic key")
        .expect("row")
        .1;
    assert_eq!(
        answer(anthropic, &need("codex", Offered::Absent)),
        Match::Short(Shortfall::Program)
    );
    // A routed need (the base-URL route through inferd) is met by programs that take one.
    let routed = |name: &str, program: &str| {
        let account = &accounts.iter().find(|(n, _)| *n == name).expect("row").1;
        answer(account, &need(program, Offered::Present))
    };
    assert_eq!(routed("anthropic key", "claude-code"), Match::Fits);
    assert_eq!(routed("openai key", "codex"), Match::Fits);
    assert_eq!(
        routed("google-ai key", "gemini-cli"),
        Match::Short(Shortfall::BaseUrl)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_agent_state_audits_a_change_once_and_a_repeat_not_at_all() {
    use porter_core::AgentState;
    use porter_core::audit::AuditEvent;
    let rig = rig_with_an_agent(AccountState::NeedsLogin).await;
    let peer = peer_as(&rig, CallerRole::AgentLauncher).await;
    let states = || {
        rig.audit
            .entries()
            .into_iter()
            .filter_map(|e| match e.event {
                AuditEvent::AgentStateSet { state } => Some((e.account, state)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    peer.set_agent_state("claude-code", "ready")
        .await
        .expect("ok");
    peer.set_agent_state("claude-code", "ready")
        .await
        .expect("again");
    assert_eq!(states(), [(Some(claude_code()), AgentState::Ready)]);
    peer.set_agent_state("claude-code", "needs_login")
        .await
        .expect("ok");
    assert_eq!(states().len(), 2);
    // A refused or invalid report writes nothing.
    let _ = peer.set_agent_state("claude-code", "bogus").await;
    let other = peer_as(&rig, CallerRole::PorterDaemon).await;
    let _ = other.set_agent_state("claude-code", "ready").await;
    assert_eq!(states().len(), 2);
}
