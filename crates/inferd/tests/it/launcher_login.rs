//! Signing an agent in and out through its launcher (from accountd's agent_login tests): the
//! launcher registers the programs it runs, inferd asks it in a signal sent to it alone, and it
//! answers with a coarse outcome. Only the agent can log itself in, so nothing of the login
//! reaches inferd.

use crate::launcher_register::error_name;
use crate::launcher_rig::*;

use inferd::launcher::LoginTiming;
use porter_core::capability::AgentProgram;
use porter_core::clock::Clock;
use porter_core::{AccountId, AccountState, LoginOutcome};
use porter_dbus::{LauncherFault, ServiceRoot};
use porter_service::audit::AuditEvent;
use porter_service::{Launchers as _, LoginEnd};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use zbus::export::futures_core::Stream;

/// inferd's clock, moved by the test.
#[derive(Debug, Default)]
struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> porter_core::UnixSeconds {
        porter_core::UnixSeconds(self.0.load(Ordering::SeqCst))
    }
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

fn agent() -> AccountId {
    AccountId::parse(AGENT).expect("id")
}

fn program() -> AgentProgram {
    AgentProgram::parse(PROGRAM).expect("program")
}

fn state_of(rig: &Rig) -> AccountState {
    rig.service
        .registry()
        .accounts
        .iter()
        .find(|a| a.id == agent())
        .map(|a| a.state)
        .expect("the account")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_goes_to_the_registrant_alone_and_ready_sets_the_account_ok() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    let launcher = rig.launcher("org.example.Launcher").await;
    let bystander = rig.launcher("org.example.Bystander").await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    let mut heard = launcher
        .proxy
        .receive_agent_login_requested()
        .await
        .expect("stream");
    let mut overheard = bystander
        .proxy
        .receive_agent_login_requested()
        .await
        .expect("stream");

    let waiting = rig
        .launchers
        .ask_login(&agent(), &program())
        .await
        .expect("a launcher is registered");
    let signal = within(next(&mut heard)).await.expect("open stream");
    let args = signal.args().expect("args");
    assert_eq!(
        (args.account.as_str(), args.program.as_str()),
        (AGENT, PROGRAM)
    );
    let request = args.request.to_owned();

    // Another launcher cannot answer it: to that connection there is no such request.
    assert_eq!(
        error_name(
            bystander
                .proxy
                .report_agent_login(&request, "ready", "")
                .await
                .expect_err("not its request")
        ),
        "org.quire.Inference1.Error.UnknownRequest"
    );
    launcher
        .proxy
        .report_agent_login(&request, "ready", "")
        .await
        .expect("reported");
    assert_eq!(state_of(&rig), AccountState::Ok);
    assert!(matches!(
        within(waiting).await,
        LoginEnd::Reported(LoginOutcome::Ready)
    ));
    // The report is taken once.
    assert_eq!(
        error_name(
            launcher
                .proxy
                .report_agent_login(&request, "ready", "")
                .await
                .expect_err("spent")
        ),
        LauncherFault::UnknownRequest.error_name_in(ServiceRoot::Inference)
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), next(&mut overheard))
            .await
            .is_err(),
        "a signal for the registrant reached another connection"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_and_a_cancel_leave_the_state_and_a_logout_done_needs_a_login() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    rig.service
        .set_agent_state(&agent(), porter_core::AgentState::Ready)
        .await
        .expect("ready");
    let launcher = rig.launcher("org.example.Launcher").await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    let mut logins = launcher
        .proxy
        .receive_agent_login_requested()
        .await
        .expect("stream");
    let mut logouts = launcher
        .proxy
        .receive_agent_logout_requested()
        .await
        .expect("stream");

    for (word, reason) in [("cancelled", ""), ("failed", "refused")] {
        let waiting = rig
            .launchers
            .ask_login(&agent(), &program())
            .await
            .expect("asked");
        let request = within(next(&mut logins))
            .await
            .expect("open")
            .args()
            .expect("args")
            .request
            .to_owned();
        launcher
            .proxy
            .report_agent_login(&request, word, reason)
            .await
            .expect("reported");
        assert!(matches!(
            within(waiting).await,
            LoginEnd::Reported(LoginOutcome::Cancelled | LoginOutcome::Failed(_))
        ));
        assert_eq!(state_of(&rig), AccountState::Ok, "{word} changes nothing");
    }

    let waiting = rig
        .launchers
        .ask_logout(&agent(), &program())
        .await
        .expect("asked");
    let request = within(next(&mut logouts))
        .await
        .expect("open")
        .args()
        .expect("args")
        .request
        .to_owned();
    // A logout request is not answered as a login.
    assert!(
        launcher
            .proxy
            .report_agent_login(&request, "ready", "")
            .await
            .is_err()
    );
    launcher
        .proxy
        .report_agent_logout(&request, "ready", "")
        .await
        .expect("done");
    assert!(matches!(
        within(waiting).await,
        LoginEnd::Reported(LoginOutcome::Ready)
    ));
    assert_eq!(state_of(&rig), AccountState::NeedsLogin);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_the_launcher_does_not_answer_expires_on_inferds_clock() {
    let clock = Arc::new(TestClock::default());
    let timing = quick_timing().with_clock(clock.clone());
    let rig = Rig::start(vec![], vec![], timing).await;
    let launcher = rig.launcher("org.example.Launcher").await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    let waiting = rig
        .launchers
        .ask_login(&agent(), &program())
        .await
        .expect("asked");
    // Nothing is decided by waiting: the bound is on the clock, which the test moves.
    clock.0.store(31, Ordering::SeqCst);
    assert!(matches!(within(waiting).await, LoginEnd::Expired));
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_launcher_for_the_program_there_is_none_to_ask_and_a_departure_ends_a_request() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    assert!(rig.launchers.ask_login(&agent(), &program()).await.is_err());

    let launcher = rig.launcher("org.example.Launcher").await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    let waiting = rig
        .launchers
        .ask_login(&agent(), &program())
        .await
        .expect("asked");
    drop(launcher);
    assert!(matches!(within(waiting).await, LoginEnd::LauncherGone));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_is_recorded_before_the_launcher_hears_it_and_the_login_is_in_no_line() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    let launcher = rig.launcher("org.example.Launcher").await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    let mut logins = launcher
        .proxy
        .receive_agent_login_requested()
        .await
        .expect("stream");
    let _waiting = rig
        .launchers
        .ask_login(&agent(), &program())
        .await
        .expect("asked");
    within(next(&mut logins)).await.expect("heard");
    let asked = rig
        .audit
        .entries()
        .iter()
        .filter(|entry| matches!(entry.event, AuditEvent::AgentLoginAsked { .. }))
        .count();
    assert_eq!(asked, 1, "the ask is in the log by the time it is heard");
    let all = format!("{:?}", rig.audit.entries());
    assert!(!all.contains(KEY));
}

/// What a wait of a test may take: the other tests' deadline.
async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(test_kit::GENEROUS, future)
        .await
        .expect("waited too long")
}
