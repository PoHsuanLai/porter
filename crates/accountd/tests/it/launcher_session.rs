//! "This session only" (from accountd's session_scope tests): the agent launcher begins and ends
//! sessions, asks the person for an agent program's key (`RequestAgentKey`, a Request object
//! answered with a permit), and a permit scoped to a session goes with it, with the process
//! credentials under it.

use crate::launcher_register::error_name;
use crate::launcher_rig::*;
use porter_ids::word::word as word_of;

use accountd::launcher::LoginTiming;
use porter_core::SessionToken;
use porter_core::audit::CredentialEnd;
use porter_core::consent::GrantScope;
use porter_dbus::Details;
use porter_dbus::accounts::launcher_fault_error_name_in;
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{ServiceRoot, Sheet};
use porter_fake::Scripted;
use porter_wire::AccountsReply;
use porter_wire::LauncherFault;
use zbus::export::futures_core::Stream;

fn session(id: &str) -> SessionToken {
    SessionToken::parse(id).expect("session")
}

fn fault(fault: LauncherFault) -> String {
    launcher_fault_error_name_in(fault, ServiceRoot::Accounts)
}

/// `RequestAgentKey` as the launcher does it, and the sheet's answer.
async fn ask_key(launcher: &Launcher, session: &str) -> Result<AccountsReply, zbus::Error> {
    let mut sheet = Sheet::subscribe_on(&launcher.connection, ServiceRoot::Accounts)
        .await
        .expect("subscribe");
    let path = launcher
        .proxy
        .request_agent_key(PROGRAM, "prompt", session, "", &sheet.options())
        .await?;
    let (code, results) = sheet.response(&path).await.expect("response");
    Ok(reply_of(SheetKind::Choose, code, results).expect("a reply"))
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_belong_to_the_connection_that_began_them() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    let first = rig.launcher("org.example.First").await;
    // No program registered yet.
    assert_eq!(
        error_name(
            first
                .proxy
                .begin_session("s1")
                .await
                .expect_err("not registered")
        ),
        fault(LauncherFault::NotRegistered)
    );
    first
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    first.proxy.begin_session("s1").await.expect("begun");
    first.proxy.begin_session("s1").await.expect("a no-op");

    let second = rig.launcher("org.example.Second").await;
    second
        .proxy
        .register_launcher(&["codex"])
        .await
        .expect("registered");
    assert_eq!(
        error_name(second.proxy.begin_session("s1").await.expect_err("taken")),
        fault(LauncherFault::SessionTaken)
    );
    assert_eq!(
        error_name(
            second
                .proxy
                .end_session("s1")
                .await
                .expect_err("not its own")
        ),
        fault(LauncherFault::UnknownSession)
    );
    first.proxy.end_session("s1").await.expect("ended");
    assert_eq!(
        error_name(
            first
                .proxy
                .end_session("s1")
                .await
                .expect_err("ended already")
        ),
        fault(LauncherFault::UnknownSession)
    );
    // A key asked for in a session the connection does not hold open is refused before a sheet.
    assert_eq!(
        error_name(
            first
                .proxy
                .request_agent_key(PROGRAM, "prompt", "s1", "", &Details::new())
                .await
                .expect_err("no such session")
        ),
        fault(LauncherFault::UnknownSession)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_permit_and_the_credential_under_it_end_with_the_session() {
    let rig = Rig::start(
        vec![],
        vec![Scripted::AllowFirst(GrantScope::Session(session("s1")))],
        LoginTiming::new(),
    )
    .await;
    let launcher = rig.launcher("org.example.Launcher").await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    launcher.proxy.begin_session("s1").await.expect("begun");
    let mut revoked = launcher
        .proxy
        .receive_process_credential_revoked()
        .await
        .expect("stream");

    let AccountsReply::Chosen(chosen) = ask_key(&launcher, "s1").await.expect("asked") else {
        panic!("the person allowed it");
    };
    let permit = chosen.grant.to_string();
    let held = rig
        .service
        .registry()
        .grants
        .iter()
        .any(|g| g.id == chosen.grant && g.scope == GrantScope::Session(session("s1")));
    assert!(held, "the permit is held for the session");

    let (id, handle) = launcher
        .proxy
        .issue_process_credential(&permit, PROGRAM)
        .await
        .expect("a session permit is accepted while its session is open");
    drop(handle);

    launcher.proxy.end_session("s1").await.expect("ended");
    assert!(
        rig.service
            .registry()
            .grants
            .iter()
            .all(|g| g.id != chosen.grant),
        "the permit goes with its session"
    );
    let signal = tokio::time::timeout(
        test_kit::GENEROUS,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut revoked).poll_next(cx)),
    )
    .await
    .expect("told in time")
    .expect("open");
    let args = signal.args().expect("args");
    assert_eq!(
        (args.id, args.reason),
        (id, word_of(&CredentialEnd::SessionClosed).to_owned())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_asked_for_an_unregistered_program_is_refused_before_a_sheet() {
    let rig = Rig::start(vec![], vec![], LoginTiming::new()).await;
    let launcher = rig.launcher("org.example.Launcher").await;
    assert_eq!(
        error_name(
            launcher
                .proxy
                .request_agent_key(PROGRAM, "prompt", "", "", &Details::new())
                .await
                .expect_err("not registered")
        ),
        fault(LauncherFault::NotRegistered)
    );
    assert!(rig.sheets.asked().is_empty());
}
