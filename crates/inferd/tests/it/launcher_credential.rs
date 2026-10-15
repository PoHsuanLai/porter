//! `IssueProcessCredential` and `RevokeProcessCredential` (from accountd's process_credential
//! tests): the agent launcher is handed an API key for a process it spawns, on a sealed memfd or
//! in a 0600 file, under the person's permit of the account to `org.quire.Agent.<program>`.
//! Nobody else may ask; the key is in no audit line; and it ends with the launcher, the permit or
//! the account.

use crate::launcher_register::error_name;
use crate::launcher_rig::*;

use inferd::launcher::LoginTiming;
use inferd::peers::Role;
use porter_core::GrantId;
use porter_core::audit::{AuditEvent, CredentialEnd};
use porter_core::consent::{Decision, GrantScope};
use porter_core::wire::Refusal;
use porter_dbus::{LauncherFault, ServiceRoot, refusal_error_name_in};
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use zbus::export::futures_core::Stream;
use zbus::zvariant::OwnedValue;

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
    fault.error_name_in(ServiceRoot::Inference)
}

fn refusal(refusal: Refusal) -> String {
    refusal_error_name_in(ServiceRoot::Inference, &refusal)
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

/// The path a `tmpfs_file` reply names.
fn path_of(handle: OwnedValue) -> std::path::PathBuf {
    String::try_from(handle).expect("a path").into()
}

fn audit_lines(rig: &Rig) -> Vec<String> {
    rig.audit
        .entries()
        .iter()
        .filter_map(|entry| match &entry.event {
            AuditEvent::ProcessCredentialIssued { handoff, .. } => {
                Some(format!("issued {}", handoff.word()))
            }
            AuditEvent::ProcessCredentialRevoked { reason, .. } => {
                Some(format!("ended {}", reason.word()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_memfd_credential_is_a_sealed_key_and_a_tmpfs_one_a_0600_file_and_the_key_is_in_no_line()
{
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;

    let (id, handle) = launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM, "memfd")
        .await
        .expect("issued");
    assert!(!id.is_empty());
    let fd = match &*handle {
        porter_dbus::zvariant::Value::Fd(fd) => std::os::fd::AsFd::as_fd(fd)
            .try_clone_to_owned()
            .expect("a descriptor"),
        other => panic!("not a descriptor: {other:?}"),
    };
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut text = String::new();
    std::fs::File::from(fd)
        .read_to_string(&mut text)
        .expect("read");
    assert_eq!(text, KEY);

    let (file_id, handle) = launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("issued");
    let path = path_of(handle);
    assert!(path.starts_with(rig.runtime.path()));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), KEY);
    let mode =
        |p: &std::path::Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().expect("dir")), 0o700);

    launcher
        .proxy
        .revoke_process_credential(&file_id)
        .await
        .expect("revoked");
    assert!(!path.exists());
    assert_eq!(
        error_name(
            launcher
                .proxy
                .revoke_process_credential(&file_id)
                .await
                .expect_err("once")
        ),
        fault(LauncherFault::UnknownCredential)
    );

    assert_eq!(
        audit_lines(&rig),
        ["issued memfd", "issued tmpfs_file", "ended process_exited"]
    );
    assert!(!format!("{:?}", rig.audit.entries()).contains(KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_is_refused_unless_every_check_holds() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    let ask = |permit: &'static str, program: &'static str, target: &'static str| {
        let proxy = launcher.proxy.clone();
        async move {
            error_name(
                proxy
                    .issue_process_credential(permit, program, target)
                    .await
                    .expect_err("refused"),
            )
        }
    };
    assert_eq!(
        ask("g-once", PROGRAM, "memfd").await,
        fault(LauncherFault::OnceGrant)
    );
    assert_eq!(
        ask("g-denied", PROGRAM, "memfd").await,
        refusal(Refusal::UnknownGrant)
    );
    assert_eq!(
        ask("g-none", PROGRAM, "memfd").await,
        refusal(Refusal::UnknownGrant)
    );
    assert_eq!(
        ask("g-other-program", PROGRAM, "memfd").await,
        refusal(Refusal::AudienceNotGranted)
    );
    assert_eq!(
        ask("g-oauth", PROGRAM, "memfd").await,
        fault(LauncherFault::NotAKeyAccount)
    );
    assert_eq!(
        ask("g-always", "codex", "memfd").await,
        fault(LauncherFault::NotRegistered)
    );
    assert_eq!(
        ask("g-always", PROGRAM, "carrier-pigeon").await,
        "org.freedesktop.DBus.Error.InvalidArgs"
    );

    // Nobody else may ask: an app, or a connection the table does not know.
    let app = rig.client_as("org.example.App", Role::App).await;
    let stranger = rig.bus.connect().await;
    for connection in [&app, &stranger] {
        let proxy = porter_dbus::LauncherProxy::new(connection)
            .await
            .expect("proxy");
        assert_eq!(
            error_name(
                proxy
                    .issue_process_credential("g-always", PROGRAM, "memfd")
                    .await
                    .expect_err("denied")
            ),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
    }
    assert!(audit_lines(&rig).is_empty(), "nothing was handed over");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permit_withdrawn_or_an_account_removed_ends_the_credential_and_tells_the_launcher() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    let mut revoked = launcher
        .proxy
        .receive_process_credential_revoked()
        .await
        .expect("stream");
    let (id, handle) = launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("issued");
    let path = path_of(handle);

    rig.service
        .revoke_grant(&GrantId::parse("g-always").expect("id"))
        .await
        .expect("withdrawn");
    rig.launchers.registry_changed().await;
    assert!(!path.exists());
    let signal = tokio::time::timeout(porter_fake::GENEROUS, next(&mut revoked))
        .await
        .expect("told in time")
        .expect("open");
    let args = signal.args().expect("args");
    assert_eq!(
        (args.id, args.reason),
        (id, CredentialEnd::GrantRevoked.word().to_owned())
    );
    assert_eq!(
        audit_lines(&rig).last().map(String::as_str),
        Some("ended grant_revoked")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_that_leaves_takes_its_credentials_with_it() {
    let rig = Rig::start(permits(), vec![], LoginTiming::new()).await;
    let launcher = registered(&rig, "org.example.Launcher").await;
    let (_, handle) = launcher
        .proxy
        .issue_process_credential("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("issued");
    let path = path_of(handle);
    drop(launcher);
    let mut ended = false;
    let deadline = porter_fake::Deadline::generous();
    while !deadline.passed() && !ended {
        ended = !path.exists();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(ended, "the file stays after its launcher left");
    assert_eq!(
        audit_lines(&rig).last().map(String::as_str),
        Some("ended launcher_gone")
    );
}
