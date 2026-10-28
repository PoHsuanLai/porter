//! The launcher's side of an agent's login and key, against a hand-written inferd
//! (`common::fake_inferd`) on a private bus: register, a stream of requests, report, sessions, and
//! the sheet that asks for an agent's key. inferd's own behaviour (who may call, the wait, the
//! permit book) is in inferd's launcher tests.
#![cfg(feature = "dbus")]

use crate::common;

use common::bus::PrivateBus;
use common::fake_inferd::{FakeInferd, Sheets};
use porter_client::{AskKind, Launcher, LauncherError};
use porter_core::capability::AgentProgram;
use porter_core::{DataClass, LauncherSession, LoginOutcome};
use porter_dbus::BusStream;
use porter_wire::Refusal;

fn program(name: &str) -> AgentProgram {
    AgentProgram::parse(name).expect("program")
}

struct Rig {
    _bus: PrivateBus,
    daemon: zbus::Connection,
    fake: FakeInferd,
    launcher: Launcher,
}

async fn rig(sheets: Sheets) -> Rig {
    let bus = PrivateBus::start();
    let daemon = bus.connect().await;
    let fake = FakeInferd::new(bus.scratch().to_path_buf(), sheets);
    fake.serve(&daemon).await;
    let launcher = Launcher::connect(&bus.connect().await)
        .await
        .expect("launcher");
    Rig {
        _bus: bus,
        daemon,
        fake,
        launcher,
    }
}

async fn next<S: BusStream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_launcher_registers_hears_a_login_and_a_logout_and_reports_each_once() {
    let rig = rig(Sheets::Hang).await;
    let mut requests = rig.launcher.requests().await.expect("requests");
    rig.launcher
        .register(&[program("claude-code")])
        .await
        .expect("registered");
    assert_eq!(
        rig.fake.seen(|seen| seen.registered.clone()),
        ["claude-code".to_owned()]
    );

    for (signal, kind, request) in [
        ("AgentLoginRequested", AskKind::Login, "req-login"),
        ("AgentLogoutRequested", AskKind::Logout, "req-logout"),
    ] {
        rig.fake
            .tell_launcher(&rig.daemon, signal, (request, "claude-code", "claude-code"))
            .await;
        let ask = tokio::time::timeout(test_kit::GENEROUS, next(&mut requests))
            .await
            .expect("a request in time")
            .expect("open stream")
            .expect("well formed");
        assert_eq!(ask.kind, kind);
        assert_eq!(ask.account.as_str(), "claude-code");
        assert_eq!(ask.program, program("claude-code"));
        assert_eq!(ask.request.as_str(), request);

        match kind {
            AskKind::Login => rig
                .launcher
                .report_login(&ask.request, LoginOutcome::Ready)
                .await
                .expect("reported"),
            AskKind::Logout => rig
                .launcher
                .report_logout(&ask.request, LoginOutcome::Ready)
                .await
                .expect("reported"),
        }
        // Said once.
        assert_eq!(
            rig.launcher
                .report_login(&ask.request, LoginOutcome::Ready)
                .await,
            Err(LauncherError::UnknownRequest)
        );
    }
    assert_eq!(
        rig.fake.seen(|seen| seen.reports.len()),
        2,
        "one report for each request"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_is_opened_and_ended_by_name() {
    let rig = rig(Sheets::Hang).await;
    let session = LauncherSession::parse("sess-1").expect("session");
    rig.launcher.begin_session(&session).await.expect("begun");
    assert_eq!(
        rig.fake.seen(|seen| seen.sessions.clone()),
        ["sess-1".to_owned()]
    );
    rig.launcher.end_session(&session).await.expect("ended");
    assert!(rig.fake.seen(|seen| seen.sessions.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_for_an_agents_key_answers_the_chosen_account_and_its_permit() {
    let rig = rig(Sheets::Permit("permit-7")).await;
    let session = LauncherSession::parse("sess-1").expect("session");
    let key = rig
        .launcher
        .request_agent_key(&program("claude-code"), DataClass::Files, Some(&session))
        .await
        .expect("a key");
    assert_eq!(key.permit.as_str(), "permit-7");
    assert_eq!(key.account.as_str(), "anthropic");
    let key = rig
        .launcher
        .request_agent_key(&program("claude-code"), DataClass::Files, None)
        .await
        .expect("a key");
    assert_eq!(key.permit.as_str(), "permit-7");
    assert_eq!(
        rig.fake.seen(|seen| seen.agent_key_requests.clone()),
        [
            (
                "claude-code".to_owned(),
                "files".to_owned(),
                "sess-1".to_owned()
            ),
            ("claude-code".to_owned(), "files".to_owned(), String::new()),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_closes_or_refuses_the_sheet_is_a_refusal() {
    for refusal in [Refusal::Dismissed, Refusal::Denied] {
        let rig = rig(Sheets::Refuse(refusal)).await;
        assert!(
            matches!(
                rig.launcher
                    .request_agent_key(&program("claude-code"), DataClass::Files, None)
                    .await,
                Err(LauncherError::Refused(got)) if got == refusal
            ),
            "{refusal:?}"
        );
    }
}
