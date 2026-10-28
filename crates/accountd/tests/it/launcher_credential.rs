//! `IssueProcessCredential` and `RevokeProcessCredential` (from accountd's process_credential
//! tests): the agent launcher is handed an API key for a process it spawns, on a sealed memfd and
//! in no file, under the person's permit of the account to `org.quire.Agent.<program>`.
//! Nobody else may ask; the key is in no audit line; and it ends with the launcher, the permit or
//! the account.

use crate::launcher_register::error_name;
use crate::launcher_rig::*;
use porter_ids::word::word as word_of;

use accountd::launcher::LoginTiming;
use porter_callers::CallerRole;
use porter_core::GrantId;
use porter_core::consent::{Decision, GrantScope};
use porter_dbus::accounts::LauncherFault;
use porter_dbus::accounts::ServiceRoot;
use porter_dbus::accounts::refusal_error_name_in;
use porter_service_core::audit::{AuditEvent, CredentialEnd};
use porter_wire::Refusal;
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::Read;
use zbus::export::futures_core::Stream;

fn permits() -> Vec<porter_core::consent::Grant> {
    use Decision::{Allow, Deny};
    use GrantScope::{Always, Once};
    vec![
        permit("g-always", agent_app(PROGRAM), KEYED, Always, Allow),
        permit("g-once", agent_app(PROGRAM), KEYED, Once, Allow),
        permit("g-denied", agent_app(PROGRAM), KEYED, Always, Deny),
        permit("g-other-program", agent_app("codex"), KEYED, Always, Allow),
        permit("g-oauth", agent_app(PROGRAM), OAUTH, Always, Allow),
    ]
}

fn fault(fault: LauncherFault) -> String {
    fault.error_name_in(ServiceRoot::Accounts)
}

fn refusal(refusal: Refusal) -> String {
    refusal_error_name_in(ServiceRoot::Accounts, &refusal)
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    std::future::poll_fn(|cx| std::pin::Pin::new(&mut *stream).poll_next(cx)).await
}

async fn registered(rig: &Rig, name: &str) -> Launcher {
    let launcher = rig.launcher(name).await;
    launcher
        .proxy
        .register_launcher(&[PROGRAM])
        .await
        .expect("registered");
    launcher
}

/// The key a reply's descriptor holds, after checking it is sealed.
fn key_of(handle: zbus::zvariant::OwnedFd) -> String {
    let fd = std::os::fd::AsFd::as_fd(&handle)
        .try_clone_to_owned()
        .expect("a descriptor");
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .expect("read");
    text
}

fn audit_lines(rig: &Rig) -> Vec<String> {
    rig.audit
        .entries()
        .iter()
        .filter_map(|entry| match &entry.event {
            AuditEvent::ProcessCredentialIssued { .. } => Some("issued".to_owned()),
            AuditEvent::ProcessCredentialRevoked { reason, .. } => {
                Some(format!("ended {}", word_of(&reason)))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_is_a_sealed_key_on_a_descriptor_and_the_key_is_in_no_line() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;

    let (id, handle) = launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM)
        .await
        .expect("issued");
    assert!(!id.is_empty());
    assert_eq!(key_of(handle), KEY);

    launcher
        .proxy
        .revoke_process_credential(&id)
        .await
        .expect("revoked");
    assert_eq!(
        error_name(
            launcher
                .proxy
                .revoke_process_credential(&id)
                .await
                .expect_err("once")
        ),
        fault(LauncherFault::UnknownCredential)
    );

    assert_eq!(audit_lines(&rig), ["issued", "ended process_exited"]);
    assert!(!format!("{:?}", rig.audit.entries()).contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_is_refused_unless_every_check_holds() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    let ask = |permit: &'static str, program: &'static str| {
        let proxy = launcher.proxy.clone();
        async move {
            error_name(
                proxy
                    .issue_process_credential(permit, program)
                    .await
                    .expect_err("refused"),
            )
        }
    };
    assert_eq!(
        ask("g-once", PROGRAM).await,
        fault(LauncherFault::OnceGrant)
    );
    assert_eq!(
        ask("g-denied", PROGRAM).await,
        refusal(Refusal::UnknownGrant)
    );
    assert_eq!(ask("g-none", PROGRAM).await, refusal(Refusal::UnknownGrant));
    assert_eq!(
        ask("g-other-program", PROGRAM).await,
        refusal(Refusal::AudienceNotGranted)
    );
    assert_eq!(
        ask("g-oauth", PROGRAM).await,
        fault(LauncherFault::NotAKeyAccount)
    );
    assert_eq!(
        ask("g-always", "codex").await,
        fault(LauncherFault::NotRegistered)
    );
    // Nobody else may ask: an app, or a connection the table does not know.
    let app = rig.client_as("org.example.App", CallerRole::App).await;
    let stranger = rig.bus.connect().await;
    for connection in [&app, &stranger] {
        let proxy = porter_dbus::accounts::LauncherProxy::new(connection)
            .await
            .expect("proxy");
        assert_eq!(
            error_name(
                proxy
                    .issue_process_credential("g-always", PROGRAM)
                    .await
                    .expect_err("denied")
            ),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
    }
    assert!(audit_lines(&rig).is_empty(), "nothing was handed over");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permit_withdrawn_ends_the_credential_and_tells_the_launcher() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    let mut revoked = launcher
        .proxy
        .receive_process_credential_revoked()
        .await
        .expect("stream");
    let (id, _) = launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM)
        .await
        .expect("issued");

    // A change the service saves: the clients are told, and a credential does not outlive it.
    rig.service
        .revoke_grant(&GrantId::parse("g-always").expect("id"))
        .await
        .expect("withdrawn");
    let signal = tokio::time::timeout(test_kit::GENEROUS, next(&mut revoked))
        .await
        .expect("told in time")
        .expect("open");
    let args = signal.args().expect("args");
    assert_eq!(
        (args.id, args.reason),
        (id, word_of(&CredentialEnd::GrantRevoked).to_owned())
    );
    assert_eq!(
        audit_lines(&rig).last().map(String::as_str),
        Some("ended grant_revoked")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_removed_ends_the_credential_as_the_accounts() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    let mut revoked = launcher
        .proxy
        .receive_process_credential_revoked()
        .await
        .expect("stream");
    launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM)
        .await
        .expect("issued");
    rig.service
        .remove_account(&porter_core::AccountId::parse(KEYED).expect("id"))
        .await
        .expect("removed");
    let signal = tokio::time::timeout(test_kit::GENEROUS, next(&mut revoked))
        .await
        .expect("told in time")
        .expect("open");
    assert_eq!(
        signal.args().expect("args").reason,
        word_of(&CredentialEnd::AccountRemoved)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_that_leaves_takes_its_credentials_with_it() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM)
        .await
        .expect("issued");
    drop(launcher);
    let deadline = test_kit::Deadline::generous();
    while !deadline.passed()
        && audit_lines(&rig).last().map(String::as_str) != Some("ended launcher_gone")
    {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        audit_lines(&rig).last().map(String::as_str),
        Some("ended launcher_gone")
    );
}
