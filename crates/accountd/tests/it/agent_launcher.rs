//! Signing an agent in and out through its launcher (lane agent-login): the launcher registers the
//! programs it runs, accountd asks it in a signal sent to it alone, and it answers with a coarse
//! outcome. Only the agent can log itself in, so nothing of the login reaches accountd.

use crate::common;

use accountd::{LoginTiming, Options};
use common::agents::*;
use common::host::send_input;
use common::*;
use porter_core::sheet::SheetInput;
use porter_core::wire::Refusal;
use porter_core::{AccountId, AccountState, AccountsReply, SecretKey, SecretPurpose};
use porter_dbus::{
    AccountProxy, CallerRole, LauncherFault, PeerProxy, Sheet, SheetKind, account_path, reply_of,
};
use porter_secrets::Secrets;
use porter_service::Clock;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use zbus::export::futures_core::Stream;
use zbus::zvariant::ObjectPath;

/// accountd's clock, moved by the test.
#[derive(Debug, Default)]
struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> porter_core::UnixSeconds {
        porter_core::UnixSeconds(self.0.load(Ordering::SeqCst))
    }
}

const BOUND: u64 = 30;

async fn rig(state: AccountState) -> (Rig, Arc<TestClock>) {
    let clock = Arc::new(TestClock::default());
    let options = Options {
        login: LoginTiming {
            clock: Some(clock.clone()),
            bound: Duration::from_secs(BOUND),
            tick: Duration::from_millis(20),
            audit: None,
        },
        ..Options::default()
    };
    let accounts = vec![
        storage_account(),
        agent_account("claude-code", state),
        agent_account("codex", state),
    ];
    let rig = Rig::start_holding(options, SheetHost::quiet(), Vec::new(), accounts).await;
    (rig, clock)
}

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

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

/// What a signal said: the request, the account, the program.
type Told = (String, String, String);

/// A launcher connection: a fake launcher process.
struct Launcher {
    /// Held so the connection lives as long as the launcher.
    _connection: zbus::Connection,
    peer: PeerProxy<'static>,
    logins: porter_dbus::AgentLoginRequestedStream,
    logouts: porter_dbus::AgentLogoutRequestedStream,
}

impl Launcher {
    async fn connect(rig: &Rig, name: &str) -> Self {
        let connection = rig.client_as(caller(name, CallerRole::AgentLauncher)).await;
        let peer = PeerProxy::new(&connection).await.expect("proxy");
        let logins = peer.receive_agent_login_requested().await.expect("stream");
        let logouts = peer.receive_agent_logout_requested().await.expect("stream");
        Self {
            _connection: connection,
            peer,
            logins,
            logouts,
        }
    }

    async fn registered(rig: &Rig, name: &str, programs: &[&str]) -> Self {
        let launcher = Self::connect(rig, name).await;
        launcher
            .peer
            .register_launcher(programs)
            .await
            .expect("registered");
        launcher
    }

    async fn login_request(&mut self) -> Told {
        let signal = tokio::time::timeout(Duration::from_secs(5), next(&mut self.logins))
            .await
            .expect("a login request in time")
            .expect("open stream");
        let args = signal.args().expect("args");
        (
            args.request.to_owned(),
            args.account.to_owned(),
            args.program.to_owned(),
        )
    }

    async fn logout_request(&mut self) -> Told {
        let signal = tokio::time::timeout(Duration::from_secs(5), next(&mut self.logouts))
            .await
            .expect("a logout request in time")
            .expect("open stream");
        let args = signal.args().expect("args");
        (
            args.request.to_owned(),
            args.account.to_owned(),
            args.program.to_owned(),
        )
    }

    /// The launcher process exits: every handle on its connection goes.
    fn leave(self) {
        drop(self);
    }

    /// Whether any request reaches this launcher within `wait`.
    async fn hears_nothing(&mut self, wait: Duration) -> bool {
        tokio::select! {
            login = next(&mut self.logins) => login.is_none(),
            logout = next(&mut self.logouts) => logout.is_none(),
            () = tokio::time::sleep(wait) => true,
        }
    }
}

/// The shell, asking an agent account to be signed in.
struct Shell {
    account: AccountProxy<'static>,
    sheet: Sheet,
}

impl Shell {
    async fn new(rig: &Rig, id: &AccountId) -> Self {
        let connection = rig
            .client_as(caller("org.quire.Shell", CallerRole::SheetHost))
            .await;
        let account = AccountProxy::builder(&connection)
            .path(
                ObjectPath::try_from(account_path(id))
                    .expect("path")
                    .into_owned(),
            )
            .expect("path")
            .build()
            .await
            .expect("account");
        let sheet = Sheet::subscribe(&connection).await.expect("subscribe");
        Self { account, sheet }
    }

    async fn ask(&self) -> zbus::zvariant::OwnedObjectPath {
        self.account
            .reauthenticate("", &self.sheet.options())
            .await
            .expect("a request")
    }

    async fn answer(&mut self, request: &zbus::zvariant::OwnedObjectPath) -> AccountsReply {
        let (code, results) =
            tokio::time::timeout(Duration::from_secs(5), self.sheet.response(request))
                .await
                .expect("a response in time")
                .expect("response");
        reply_of(SheetKind::Reauthenticate, code, results).expect("a reply")
    }
}

/// The views the sheet host was shown, in order, as their JSON.
fn views(rig: &Rig) -> Vec<String> {
    let calls = rig.host_log.calls();
    let mut shown: Vec<String> = calls
        .opened
        .iter()
        .map(|(_, _, view)| view.clone())
        .collect();
    shown.extend(calls.updated.iter().map(|(_, view)| view.clone()));
    shown
}

fn last_view(rig: &Rig) -> String {
    let calls = rig.host_log.calls();
    calls
        .updated
        .last()
        .map(|(_, view)| view.clone())
        .or_else(|| calls.opened.last().map(|(_, _, view)| view.clone()))
        .unwrap_or_default()
}

async fn dismiss(rig: &Rig) {
    let handle = rig
        .host_log
        .calls()
        .opened
        .last()
        .expect("a sheet")
        .0
        .clone();
    send_input(&rig.host_connection, &handle, &SheetInput::Dismiss).await;
}

async fn retry(rig: &Rig) {
    let handle = rig
        .host_log
        .calls()
        .opened
        .last()
        .expect("a sheet")
        .0
        .clone();
    send_input(&rig.host_connection, &handle, &SheetInput::Retry).await;
}

const INVALID: &str = "org.freedesktop.DBus.Error.InvalidArgs";

fn fault_name(fault: LauncherFault) -> String {
    fault.error_name()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_has_one_launcher_first_wins_and_a_dropped_connection_frees_it() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let first = Launcher::registered(&rig, "org.example.First", &["claude-code"]).await;
    let second = Launcher::connect(&rig, "org.example.Second").await;

    let refused = second
        .peer
        .register_launcher(&["claude-code"])
        .await
        .expect_err("taken");
    assert_eq!(
        error_name(&refused),
        fault_name(LauncherFault::AlreadyRegistered)
    );
    // Asking for a held program among free ones takes none of them.
    let refused = second
        .peer
        .register_launcher(&["gemini-cli", "claude-code"])
        .await
        .expect_err("taken");
    assert_eq!(
        error_name(&refused),
        fault_name(LauncherFault::AlreadyRegistered)
    );
    second
        .peer
        .register_launcher(&["gemini-cli"])
        .await
        .expect("gemini-cli was left free");

    // The holder may say it again, and add more later.
    first
        .peer
        .register_launcher(&["claude-code"])
        .await
        .expect("a no-op");
    first
        .peer
        .register_launcher(&["claude-code", "codex"])
        .await
        .expect("more programs");
    let refused = second
        .peer
        .register_launcher(&["codex"])
        .await
        .expect_err("taken now");
    assert_eq!(
        error_name(&refused),
        fault_name(LauncherFault::AlreadyRegistered)
    );

    // The registration lives as long as the connection.
    first.leave();
    let mut freed = false;
    for _ in 0..250 {
        if second
            .peer
            .register_launcher(&["claude-code", "codex"])
            .await
            .is_ok()
        {
            freed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(freed, "the program was never freed");
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_launcher_registers_or_answers_and_bad_words_are_invalid() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    for who in [
        CallerRole::App,
        CallerRole::Settings,
        CallerRole::SheetHost,
        CallerRole::PorterDaemon,
        CallerRole::Agent,
        CallerRole::Cua,
    ] {
        let connection = rig.client_as(caller("org.example.Other", who)).await;
        let peer = PeerProxy::new(&connection).await.expect("proxy");
        for refused in [
            peer.register_launcher(&["claude-code"]).await,
            peer.report_agent_login("login-1", "ready", "").await,
            peer.report_agent_logout("login-1", "ready", "").await,
        ] {
            assert_eq!(
                error_name(&refused.expect_err("refused")),
                ACCESS_DENIED,
                "{who:?}"
            );
        }
    }
    let stranger = rig.stranger().await;
    let peer = PeerProxy::new(&stranger).await.expect("proxy");
    let refused = peer
        .register_launcher(&["claude-code"])
        .await
        .expect_err("refused");
    assert_eq!(error_name(&refused), ACCESS_DENIED);

    let launcher = Launcher::connect(&rig, "org.example.Launcher").await;
    for programs in [&[][..], &["Not A Program"][..], &["a b"][..], &[""][..]] {
        let refused = launcher
            .peer
            .register_launcher(programs)
            .await
            .expect_err("invalid");
        assert_eq!(error_name(&refused), INVALID, "{programs:?}");
    }
    for (request, outcome, reason) in [
        ("Not An Id", "ready", ""),
        ("login-1", "done", ""),
        ("login-1", "ready", "refused"),
        ("login-1", "failed", ""),
        (
            "login-1",
            "failed",
            "https://example.org/device?code=ABCD-EFGH",
        ),
        ("login-1", "cancelled", "other"),
    ] {
        for refused in [
            launcher
                .peer
                .report_agent_login(request, outcome, reason)
                .await,
            launcher
                .peer
                .report_agent_logout(request, outcome, reason)
                .await,
        ] {
            assert_eq!(
                error_name(&refused.expect_err("invalid")),
                INVALID,
                "{request}/{outcome}/{reason}"
            );
        }
    }
    // A well-formed report for nothing is no request.
    let refused = launcher
        .peer
        .report_agent_login("login-1", "ready", "")
        .await
        .expect_err("no such request");
    assert_eq!(
        error_name(&refused),
        fault_name(LauncherFault::UnknownRequest)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_goes_to_the_registrant_alone_and_ready_sets_the_account_ok() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    // Another launcher that registered another program, and one that registered nothing.
    let mut codex = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let mut idle = Launcher::connect(&rig, "org.example.Idle").await;

    let settings = settings(&rig).await;
    let mut changes = settings.changes().await.expect("changes");
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;

    let (id, account, program) = launcher.login_request().await;
    assert_eq!(
        (account.as_str(), program.as_str()),
        ("claude-code", "claude-code")
    );
    assert!(codex.hears_nothing(Duration::from_millis(400)).await);
    assert!(idle.hears_nothing(Duration::from_millis(100)).await);

    // The sheet shows Working while the agent does it, and the account waits.
    eventually("the sheet to open", || !views(&rig).is_empty()).await;
    assert!(views(&rig)[0].contains("working"), "{:?}", views(&rig));
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);

    launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect("reported");
    assert_eq!(shell.answer(&request).await, AccountsReply::Reauthenticated);
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
    assert!(last_view(&rig).contains("done"), "{}", last_view(&rig));
    // Saved with the registry, and Settings is told.
    let saved = rig.store.stored().expect("saved").accounts;
    assert!(
        saved
            .iter()
            .any(|a| a.id == claude_code() && a.state == AccountState::Ok)
    );
    let change = tokio::time::timeout(Duration::from_secs(5), next(&mut changes))
        .await
        .expect("a Changed in time")
        .expect("open stream")
        .expect("a change");
    assert_eq!(change.key, key("accounts.claude-code.state"));
    assert_eq!(change.value, toml::Value::String("ok".into()));
    // Codex's account is its own.
    assert_eq!(
        state_of(&rig, &AccountId::parse("codex").expect("id")),
        AccountState::NeedsLogin
    );
    // An answered request is gone: saying it again is no request.
    let again = launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect_err("answered");
    assert_eq!(
        error_name(&again),
        fault_name(LauncherFault::UnknownRequest)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_a_cancel_and_an_expiry_leave_needs_login_and_the_sheet_shows_which() {
    // (what the launcher says, the sheet's fault word, what the caller is told)
    let table: [(&str, &str, &str, Refusal); 4] = [
        ("failed", "refused", "refused", Refusal::Denied),
        (
            "failed",
            "not_installed",
            "not_installed",
            Refusal::Unavailable,
        ),
        ("failed", "unreachable", "unreachable", Refusal::Unavailable),
        ("cancelled", "", "cancelled", Refusal::Dismissed),
    ];
    for (outcome, reason, shown, refusal) in table {
        let (rig, _) = rig(AccountState::NeedsLogin).await;
        let mut launcher =
            Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
        let mut shell = Shell::new(&rig, &claude_code()).await;
        let request = shell.ask().await;
        let (id, _, _) = launcher.login_request().await;
        launcher
            .peer
            .report_agent_login(&id, outcome, reason)
            .await
            .expect("reported");
        eventually("the outcome on the sheet", || {
            last_view(&rig).contains("failed")
        })
        .await;
        let view = last_view(&rig);
        assert!(
            view.contains(&format!("\"fault\":\"{shown}\"")),
            "{outcome}: {view}"
        );
        assert_eq!(
            state_of(&rig, &claude_code()),
            AccountState::NeedsLogin,
            "{outcome}"
        );
        // The person closes the failed sheet.
        dismiss(&rig).await;
        assert_eq!(
            shell.answer(&request).await,
            AccountsReply::Refused(refusal),
            "{outcome}/{reason}"
        );
        assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_the_launcher_does_not_answer_expires_on_accountds_clock() {
    let (rig, clock) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    let (id, _, _) = launcher.login_request().await;

    // Inside the bound nothing happens, however long the wall clock runs.
    clock
        .0
        .store(i64::try_from(BOUND).expect("bound") - 1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!last_view(&rig).contains("expired"), "{}", last_view(&rig));

    clock
        .0
        .store(i64::try_from(BOUND).expect("bound"), Ordering::SeqCst);
    eventually("the sheet to say it expired", || {
        last_view(&rig).contains("expired")
    })
    .await;
    assert!(last_view(&rig).contains("\"fault\":\"expired\""));
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    dismiss(&rig).await;
    assert_eq!(
        shell.answer(&request).await,
        AccountsReply::Refused(Refusal::Unavailable)
    );
    // Too late to answer it.
    let late = launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect_err("expired");
    assert_eq!(error_name(&late), fault_name(LauncherFault::UnknownRequest));
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_launcher_for_the_program_the_refusal_is_no_launcher() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    // A launcher of another program is no launcher of this one.
    let mut other = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    eventually("the sheet to say there is no launcher", || {
        last_view(&rig).contains("no_launcher")
    })
    .await;
    assert!(other.hears_nothing(Duration::from_millis(200)).await);
    dismiss(&rig).await;
    let reply = shell.answer(&request).await;
    assert_eq!(reply, AccountsReply::Refused(Refusal::NoLauncher));
    assert_eq!(
        refusal_name(Refusal::NoLauncher),
        "org.quire.Accounts1.Error.NoLauncher"
    );
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_on_a_failed_sheet_asks_the_launcher_again() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    let (first, _, _) = launcher.login_request().await;
    launcher
        .peer
        .report_agent_login(&first, "failed", "other")
        .await
        .expect("reported");
    eventually("the failure", || last_view(&rig).contains("failed")).await;
    retry(&rig).await;
    let (second, _, _) = launcher.login_request().await;
    assert_ne!(first, second);
    launcher
        .peer
        .report_agent_login(&second, "ready", "")
        .await
        .expect("reported");
    assert_eq!(shell.answer(&request).await, AccountsReply::Reauthenticated);
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_connection_that_got_the_request_may_answer_it() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let rival = Launcher::registered(&rig, "org.example.Rival", &["codex"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    let (id, _, _) = launcher.login_request().await;

    // Another launcher, with a real request id in hand, is refused and changes nothing.
    for outcome in [("ready", ""), ("cancelled", ""), ("failed", "other")] {
        let refused = rival
            .peer
            .report_agent_login(&id, outcome.0, outcome.1)
            .await
            .expect_err("not its request");
        assert_eq!(
            error_name(&refused),
            fault_name(LauncherFault::UnknownRequest)
        );
    }
    // So is a logout report for a login request, from the right connection.
    let refused = launcher
        .peer
        .report_agent_logout(&id, "ready", "")
        .await
        .expect_err("not a logout");
    assert_eq!(
        error_name(&refused),
        fault_name(LauncherFault::UnknownRequest)
    );
    // A word outside the set does not use the request up.
    let refused = launcher
        .peer
        .report_agent_login(&id, "ready", "x")
        .await
        .expect_err("invalid");
    assert_eq!(error_name(&refused), INVALID);
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);

    launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect("the registrant may");
    assert_eq!(shell.answer(&request).await, AccountsReply::Reauthenticated);
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_sheet_does_not_withdraw_the_request() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    let (id, _, _) = launcher.login_request().await;
    eventually("the sheet to open", || !views(&rig).is_empty()).await;
    dismiss(&rig).await;
    assert_eq!(
        shell.answer(&request).await,
        AccountsReply::Refused(Refusal::Dismissed)
    );
    // The agent finishes anyway, and the account follows it.
    launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect("still pending");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_that_leaves_with_a_request_pending_ends_it() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    let _ = launcher.login_request().await;
    launcher.leave();
    eventually("the sheet to say the launcher is gone", || {
        last_view(&rig).contains("no_launcher")
    })
    .await;
    dismiss(&rig).await;
    assert_eq!(
        shell.answer(&request).await,
        AccountsReply::Refused(Refusal::NoLauncher)
    );
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_out_asks_the_launcher_and_only_a_done_report_forgets_the_sign_in() {
    let (rig, _) = rig(AccountState::Ok).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");
    let sign_out = key("accounts.claude-code.sign_out");

    client.invoke(&sign_out).await.expect("asked");
    let (id, account, program) = launcher.logout_request().await;
    assert_eq!(
        (account.as_str(), program.as_str()),
        ("claude-code", "claude-code")
    );
    // No login request went with it, and the state waits for the report.
    assert!(launcher.hears_nothing(Duration::from_millis(100)).await);
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
    assert_eq!(sign_out_news(&mut changes).await, "asked");

    // A failure leaves it signed in.
    launcher
        .peer
        .report_agent_logout(&id, "failed", "other")
        .await
        .expect("reported");
    assert_eq!(sign_out_news(&mut changes).await, "failed");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);

    // A done report signs it out.
    client.invoke(&sign_out).await.expect("asked");
    let (id, _, _) = launcher.logout_request().await;
    assert_eq!(sign_out_news(&mut changes).await, "asked");
    launcher
        .peer
        .report_agent_logout(&id, "ready", "")
        .await
        .expect("reported");
    assert_eq!(sign_out_news(&mut changes).await, "done");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    // The account stays; only its state moved.
    assert_eq!(rig.service.registry().accounts.len(), 3);

    // Cancelled is told as cancelled.
    client.invoke(&sign_out).await.expect("asked");
    let (id, _, _) = launcher.logout_request().await;
    launcher
        .peer
        .report_agent_logout(&id, "cancelled", "")
        .await
        .expect("reported");
    let mut words = Vec::new();
    for _ in 0..2 {
        words.push(sign_out_news(&mut changes).await);
    }
    assert_eq!(words, ["asked", "cancelled"]);
}

/// The next word said on the sign out row.
async fn sign_out_news(changes: &mut ds_settings::live::Changes) -> String {
    loop {
        let change = tokio::time::timeout(Duration::from_secs(5), next(changes))
            .await
            .expect("a Changed in time")
            .expect("open stream")
            .expect("a change");
        // The Set itself answers with the row's own value (a button, `false`); the words are text.
        if change.key == key("accounts.claude-code.sign_out") && change.value.is_str() {
            return change
                .value
                .as_str()
                .unwrap_or_else(|| panic!("a word, got {:?}", change.value))
                .to_owned();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_out_with_no_launcher_forgets_the_state_and_says_the_login_was_not_touched() {
    let (rig, _) = rig(AccountState::Ok).await;
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");
    client
        .invoke(&key("accounts.claude-code.sign_out"))
        .await
        .expect("signed out");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::NeedsLogin);
    assert_eq!(sign_out_news(&mut changes).await, "login_untouched");

    // A launcher of another program does not change that.
    let mut codex = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    client
        .invoke(&key("accounts.claude-code.sign_out"))
        .await
        .expect("signed out");
    assert_eq!(sign_out_news(&mut changes).await, "login_untouched");
    assert!(codex.hears_nothing(Duration::from_millis(200)).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_out_the_launcher_never_answers_expires_and_a_departure_ends_it() {
    let (rig, clock) = rig(AccountState::Ok).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let client = settings(&rig).await;
    let mut changes = client.changes().await.expect("changes");
    let sign_out = key("accounts.claude-code.sign_out");

    client.invoke(&sign_out).await.expect("asked");
    let (id, _, _) = launcher.logout_request().await;
    assert_eq!(sign_out_news(&mut changes).await, "asked");
    clock
        .0
        .store(i64::try_from(BOUND).expect("bound"), Ordering::SeqCst);
    assert_eq!(sign_out_news(&mut changes).await, "expired");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
    let late = launcher
        .peer
        .report_agent_logout(&id, "ready", "")
        .await
        .expect_err("expired");
    assert_eq!(error_name(&late), fault_name(LauncherFault::UnknownRequest));

    client.invoke(&sign_out).await.expect("asked");
    let _ = launcher.logout_request().await;
    assert_eq!(sign_out_news(&mut changes).await, "asked");
    launcher.leave();
    assert_eq!(sign_out_news(&mut changes).await, "launcher_gone");
    assert_eq!(state_of(&rig, &claude_code()), AccountState::Ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bus_carries_the_outcome_and_nothing_of_the_login() {
    // What the agent holds while it logs itself in. The fake launcher never puts any of it on the
    // bus; the scan checks that nothing accountd sent or stored carries such a thing either, and
    // that the only words in the requests are ids and names.
    const TOKEN: &str = "sk-ant-oat01-AGENT-LOGIN-CANARY";
    const URL: &str = "https://claude.ai/oauth/authorize?code=CANARY-CODE";
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut tap = Tap::start(&rig.bus).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;

    let request = shell.ask().await;
    let (id, account, program) = launcher.login_request().await;
    // The agent opens its browser and takes its code on its own, off the bus.
    let _agent_holds = (TOKEN, URL);
    launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect("reported");
    assert_eq!(shell.answer(&request).await, AccountsReply::Reauthenticated);

    let client = settings(&rig).await;
    client
        .invoke(&key("accounts.claude-code.sign_out"))
        .await
        .expect("asked");
    let (out, _, _) = launcher.logout_request().await;
    launcher
        .peer
        .report_agent_logout(&out, "ready", "")
        .await
        .expect("reported");

    let seen = tap.drain().await;
    assert!(seen.len() > 5, "the tap saw {} messages", seen.len());
    for bytes in &seen {
        for needle in [
            TOKEN, URL, "CANARY", "sk-ant", "https://", "oauth", "Bearer",
        ] {
            assert!(!contains(bytes, needle), "{needle} on the bus");
        }
    }
    // What was said of the request is its id, the account and the program.
    assert!(id.starts_with("login-") && account == "claude-code" && program == "claude-code");
    // Nor does the registry or the audit trail hold more than the state.
    let saved = rig
        .store
        .stored()
        .expect("saved")
        .accounts
        .iter()
        .filter(|a| a.id == claude_code())
        .map(|a| serde_json::to_string(a).expect("json"))
        .collect::<String>();
    assert!(saved.contains("claude-code"), "the account was saved");
    for needle in ["CANARY", "sk-ant", "https://claude", "secret", "password"] {
        assert!(
            !saved.to_lowercase().contains(&needle.to_lowercase()),
            "{needle} in registry"
        );
    }
    for purpose in [
        SecretPurpose::Password,
        SecretPurpose::OAuthRefresh,
        SecretPurpose::ApiKey,
    ] {
        let key = SecretKey {
            account: claude_code(),
            purpose,
        };
        assert!(
            rig.secrets.get(&key).await.is_err(),
            "no secret was filed for an agent that signs itself in ({purpose:?})"
        );
    }
}

/// The agent events in the audit, as (event name, account, state or program, request) words.
fn agent_audit(rig: &Rig) -> Vec<String> {
    use porter_core::audit::AuditEvent;
    rig.audit
        .entries()
        .into_iter()
        .filter_map(|e| {
            let account = e.account.map(|a| a.as_str().to_owned()).unwrap_or_default();
            match e.event {
                AuditEvent::AgentLoginAsked { request, program } => Some(format!(
                    "login_asked {account} {} {}",
                    program.as_str(),
                    request.as_str()
                )),
                AuditEvent::AgentLogoutAsked { request, program } => Some(format!(
                    "logout_asked {account} {} {}",
                    program.as_str(),
                    request.as_str()
                )),
                AuditEvent::AgentStateSet { state } => Some(format!(
                    "state {account} {}",
                    serde_json::to_string(&state)
                        .expect("json")
                        .trim_matches('"')
                )),
                _ => None,
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn login_and_logout_requests_and_their_state_changes_are_audited_without_the_login() {
    let (rig, _) = rig(AccountState::NeedsLogin).await;
    let mut launcher = Launcher::registered(&rig, "org.example.Launcher", &["claude-code"]).await;
    let mut shell = Shell::new(&rig, &claude_code()).await;
    let request = shell.ask().await;
    let (id, _, _) = launcher.login_request().await;
    assert_eq!(
        agent_audit(&rig),
        [format!("login_asked claude-code claude-code {id}")]
    );
    launcher
        .peer
        .report_agent_login(&id, "ready", "")
        .await
        .expect("reported");
    assert_eq!(shell.answer(&request).await, AccountsReply::Reauthenticated);
    assert_eq!(
        agent_audit(&rig).last().map(String::as_str),
        Some("state claude-code ready")
    );

    // A sign out: asked, then a failed report changes nothing, a done report sets needs_login.
    let client = settings(&rig).await;
    let sign_out = key("accounts.claude-code.sign_out");
    client.invoke(&sign_out).await.expect("asked");
    let (out, _, _) = launcher.logout_request().await;
    launcher
        .peer
        .report_agent_logout(&out, "failed", "other")
        .await
        .expect("reported");
    let before = agent_audit(&rig);
    assert_eq!(before.len(), 3, "{before:?}");
    assert_eq!(
        before[2],
        format!("logout_asked claude-code claude-code {out}")
    );
    client.invoke(&sign_out).await.expect("asked");
    let (out, _, _) = launcher.logout_request().await;
    launcher
        .peer
        .report_agent_logout(&out, "ready", "")
        .await
        .expect("reported");
    eventually("the state line", || agent_audit(&rig).len() == 5).await;
    assert_eq!(agent_audit(&rig)[4], "state claude-code needs_login");

    // Nothing of the login: the file holds ids and words only.
    let text = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(!text.contains("other"), "{text}");
}
