//! `Tokens.IssueProcessCredential` and `Tokens.RevokeProcessCredential` (agent-session ask P2): the
//! agent launcher is handed an API key for a process it spawns, on a sealed memfd or in a 0600
//! file, under the person's grant of the account to `org.quire.Agent.<program>`. Nobody else may
//! ask; the key is in no reply body, signal, audit line or log; and it ends with the launcher, the
//! grant or the account.

use crate::common;

use accountd::Options;
use common::*;
use porter_core::audit::{AuditEntry, AuditEvent, CredentialEnd, Handoff};
use porter_core::consent::{Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountId, AccountState, AppId, AppName, Audience, AuthKind, CapabilityKind,
    Credential, DataClass, GrantId, Isolation, SecretKey, SecretPurpose, SecretText, SpaceScope,
    UnixSeconds,
};
use porter_dbus::zvariant::{OwnedValue, Value};
use porter_dbus::{CallerRole, GrantsProxy, LauncherFault, PeerProxy, TokensProxy};
use porter_fake::llm_account;
use porter_secrets::Secrets;
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use zbus::export::futures_core::Stream;

const KEY: &str = "sk-ant-api03-S3CRET-HANDOFF-KEY-0123456789";
const OTHER_KEY: &str = "sk-oauth-S3CRET-NOT-FOR-HANDOFF";
const PROGRAM: &str = "claude-code";

/// The account that holds an API key, and the one that does not.
const KEYED: &str = "anthropic";
const OAUTH: &str = "oauth-llm";

/// A runtime directory of its own, removed on drop.
struct Runtime(PathBuf);

impl Runtime {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "runtime-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("runtime dir");
        Self(dir)
    }

    fn agent_dir(&self) -> PathBuf {
        self.0.join("porter").join("agent")
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn account(id: &str, auth: AuthKind) -> Account {
    Account {
        id: AccountId::parse(id).expect("id"),
        auth,
        ..llm_account()
    }
}

fn agent_app(program: &str) -> AppId {
    AppId {
        name: AppName::parse(&format!("org.quire.Agent.{program}")).expect("name"),
        isolation: Isolation::Unsandboxed,
    }
}

fn grant(id: &str, app: AppId, account: &str, scope: GrantScope, decision: Decision) -> Grant {
    Grant {
        id: GrantId::parse(id).expect("id"),
        key: GrantKey {
            app,
            account: AccountId::parse(account).expect("id"),
            kind: CapabilityKind::Llm,
            class: DataClass::Prompt,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision,
        scope,
        at: UnixSeconds(1),
    }
}

/// What the person has granted, by grant id.
fn grants() -> Vec<Grant> {
    use Decision::{Allow, Deny};
    use GrantScope::{Always, Once};
    let companion = app("org.quire.Companion");
    vec![
        grant("g-always", agent_app(PROGRAM), KEYED, Always, Allow),
        grant("g-second", agent_app(PROGRAM), KEYED, Always, Allow),
        grant("g-once", agent_app(PROGRAM), KEYED, Once, Allow),
        grant("g-denied", agent_app(PROGRAM), KEYED, Always, Deny),
        grant("g-other-app", companion, KEYED, Always, Allow),
        grant("g-other-program", agent_app("codex"), KEYED, Always, Allow),
        grant("g-oauth", agent_app(PROGRAM), OAUTH, Always, Allow),
    ]
}

fn secret(account: &str) -> SecretKey {
    SecretKey {
        account: AccountId::parse(account).expect("id"),
        purpose: SecretPurpose::ApiKey,
    }
}

/// accountd with the two accounts, the grants above, both keys filed and `runtime` as the
/// runtime directory (none: the memfd way only).
async fn rig(runtime: Option<&Runtime>) -> Rig {
    let options = Options {
        runtime_dir: runtime.map(|r| r.0.clone()),
        ..Options::default()
    };
    let accounts = vec![
        storage_account(),
        account(KEYED, AuthKind::ApiKey),
        account(OAUTH, AuthKind::OAuthPkce),
    ];
    let providers = vec![porter_fake::cloud_provider(), porter_fake::llm_provider()];
    let rig = Rig::start_granted(options, SheetHost::quiet(), providers, accounts, grants()).await;
    for (account, key) in [(KEYED, KEY), (OAUTH, OTHER_KEY)] {
        rig.secrets
            .put(&secret(account), &Credential::ApiKey(SecretText::new(key)))
            .await
            .expect("key filed");
    }
    rig
}

/// A fake launcher process: a connection accountd knows as `AgentLauncher`.
struct Launcher {
    connection: zbus::Connection,
    tokens: TokensProxy<'static>,
    revoked: porter_dbus::ProcessCredentialRevokedStream,
}

impl Launcher {
    async fn connect(rig: &Rig, name: &str) -> Self {
        let connection = rig.client_as(caller(name, CallerRole::AgentLauncher)).await;
        let tokens = TokensProxy::new(&connection).await.expect("proxy");
        let revoked = tokens
            .receive_process_credential_revoked()
            .await
            .expect("stream");
        Self {
            connection,
            tokens,
            revoked,
        }
    }

    async fn registered(rig: &Rig, name: &str, programs: &[&str]) -> Self {
        let launcher = Self::connect(rig, name).await;
        PeerProxy::new(&launcher.connection)
            .await
            .expect("proxy")
            .register_launcher(programs)
            .await
            .expect("registered");
        launcher
    }

    async fn issue(&self, grant: &str, program: &str, target: &str) -> Result<Issued, zbus::Error> {
        let (id, handle) = self
            .tokens
            .issue_process_credential(grant, program, target)
            .await?;
        Ok(Issued { id, handle })
    }

    /// The next signal within `wait`: the credential and the reason.
    async fn told(&mut self, wait: Duration) -> Option<(String, String)> {
        let next = tokio::time::timeout(
            wait,
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut self.revoked).poll_next(cx)),
        )
        .await
        .ok()??;
        let args = next.args().expect("args");
        Some((args.id.to_owned(), args.reason.to_owned()))
    }

    /// The launcher process exits: every handle on its connection goes.
    fn leave(self) {
        drop(self);
    }
}

#[derive(Debug)]
struct Issued {
    id: String,
    handle: OwnedValue,
}

impl Issued {
    fn fd(&self) -> std::fs::File {
        match &*self.handle {
            Value::Fd(fd) => std::fs::File::from(fd.as_fd().try_clone_to_owned().expect("dup")),
            other => panic!("a descriptor, not {other:?}"),
        }
    }

    fn path(&self) -> PathBuf {
        match &*self.handle {
            Value::Str(path) => PathBuf::from(path.as_str()),
            other => panic!("a path, not {other:?}"),
        }
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

fn fault(fault: LauncherFault) -> String {
    fault.error_name()
}

fn handoffs(rig: &Rig) -> Vec<AuditEntry> {
    rig.audit
        .entries()
        .into_iter()
        .filter(|e| {
            matches!(
                e.event,
                AuditEvent::ProcessCredentialIssued { .. }
                    | AuditEvent::ProcessCredentialRevoked { .. }
            )
        })
        .collect()
}

fn issued_event(handoff: Handoff) -> AuditEvent {
    AuditEvent::ProcessCredentialIssued {
        audience: Audience("org.quire.Agent.claude-code".into()),
        handoff,
    }
}

fn revoked_event(reason: CredentialEnd) -> AuditEvent {
    AuditEvent::ProcessCredentialRevoked {
        audience: Audience("org.quire.Agent.claude-code".into()),
        reason,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_memfd_credential_is_the_key_on_a_sealed_descriptor_and_the_key_is_nowhere_else() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let mut tap = Tap::start(&rig.bus).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;

    let issued = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect("issued");
    assert_eq!(issued.id, "cred-1");
    let fd = issued.fd();
    assert_eq!(
        fcntl_get_seals(&fd).expect("seals"),
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
    );
    let mut file = fd;
    let mut text = String::new();
    file.read_to_string(&mut text).expect("readable");
    assert_eq!(text, KEY);
    assert!(file.write_all(b"x").is_err(), "sealed against writes");

    // One audit line, naming the app the grant is for and the account, never the key.
    let lines = handoffs(&rig);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].event, issued_event(Handoff::Memfd));
    assert_eq!(lines[0].app, Some(agent_app(PROGRAM)));
    assert_eq!(lines[0].account, Some(AccountId::parse(KEYED).expect("id")));

    // A memfd leaves no file, and no directory is made for it.
    assert!(!runtime.agent_dir().exists());

    // Nothing on the bus so far carries the key.
    let seen = tap.drain().await;
    assert!(
        seen.len() > 3,
        "the monitor saw the traffic: {}",
        seen.len()
    );
    assert!(
        seen.iter().all(|m| !contains(m, KEY)),
        "the key is on the bus"
    );
    assert!(
        seen.iter().any(|m| contains(m, "g-always")),
        "positive control: the scan sees values that do cross"
    );
    // The positive control: the scan does find the key when it is sent as an argument on the bus
    // (the call, and the error that names the argument), so a miss above means absent, not blind.
    let _ = launcher.issue(KEY, PROGRAM, "memfd").await;
    let control = tap.drain().await;
    assert!(
        control.iter().any(|m| contains(m, KEY)),
        "the scan finds a key that does cross"
    );
    // Neither the audit, the sheet host's log, nor the other account's key.
    let audited = serde_json::to_string(&rig.audit.entries()).expect("json");
    assert!(
        !audited.contains(KEY) && !audited.contains(OTHER_KEY),
        "{audited}"
    );
    assert!(!format!("{:?}", rig.host_log.calls()).contains(KEY));
    assert!(seen.iter().all(|m| !contains(m, OTHER_KEY)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tmpfs_credential_is_a_0600_file_in_a_0700_directory_and_revoking_it_unlinks_it() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let mut tap = Tap::start(&rig.bus).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;

    let issued = launcher
        .issue("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("issued");
    let path = issued.path();
    assert_eq!(
        path,
        runtime.agent_dir().join(&issued.id).join("key"),
        "$XDG_RUNTIME_DIR/porter/agent/<credential>/key"
    );
    assert_eq!(std::fs::read_to_string(&path).expect("read"), KEY);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().expect("dir")), 0o700);
    assert_eq!(mode(&runtime.agent_dir()), 0o700);
    assert_eq!(handoffs(&rig)[0].event, issued_event(Handoff::TmpfsFile));

    // The path crossed the bus; the key did not.
    let seen = tap.drain().await;
    assert!(seen.iter().any(|m| contains(m, &issued.id)));
    assert!(
        seen.iter().all(|m| !contains(m, KEY)),
        "the key is on the bus"
    );

    // The launcher ends it: the file and its directory go, and the end is audited.
    launcher
        .tokens
        .revoke_process_credential(&issued.id)
        .await
        .expect("revoked");
    assert!(!path.exists());
    assert!(!path.parent().expect("dir").exists());
    let lines = handoffs(&rig);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1].event, revoked_event(CredentialEnd::ProcessExited));
    // The launcher knows; it is not signalled for what it did itself.
    let mut launcher = launcher;
    assert_eq!(launcher.told(Duration::from_millis(300)).await, None);
    // Once.
    let err = launcher
        .tokens
        .revoke_process_credential(&issued.id)
        .await
        .expect_err("gone");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownCredential));
    assert_eq!(handoffs(&rig).len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_launcher_of_a_program_may_ask_for_its_key() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let _first = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;

    // Every other role, and a stranger: AccessDenied, for both methods.
    for refused in [
        rig.client_as(caller("org.quire.Agent.claude-code", CallerRole::App))
            .await,
        rig.client_as(caller("org.quire.Agent.claude-code", CallerRole::Agent))
            .await,
        rig.client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
            .await,
        rig.client_as(caller("org.quire.Settings", CallerRole::Settings))
            .await,
        rig.client_as(caller("org.example.Sheets", CallerRole::SheetHost))
            .await,
        rig.stranger().await,
    ] {
        let tokens = TokensProxy::new(&refused).await.expect("proxy");
        let err = tokens
            .issue_process_credential("g-always", PROGRAM, "memfd")
            .await
            .expect_err("refused");
        assert_eq!(error_name(&err), ACCESS_DENIED);
        let err = tokens
            .revoke_process_credential("cred-1")
            .await
            .expect_err("refused");
        assert_eq!(error_name(&err), ACCESS_DENIED);
    }

    // A launcher that holds no registration of the program, or another program's.
    let bare = Launcher::connect(&rig, "org.example.Bare").await;
    let err = bare
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect_err("not registered");
    assert_eq!(error_name(&err), fault(LauncherFault::NotRegistered));
    let other = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let err = other
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect_err("not its program");
    assert_eq!(error_name(&err), fault(LauncherFault::NotRegistered));

    // Nothing was issued and nothing audited.
    assert!(handoffs(&rig).is_empty());
    assert!(!runtime.agent_dir().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_that_does_not_back_a_process_credential_is_refused_with_its_reason() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM, "codex"]).await;
    let refusal = refusal_name;
    let cases = [
        // A once grant: the first use would spend it, the key would outlive it.
        ("g-once", PROGRAM, fault(LauncherFault::OnceGrant)),
        // A grant for another app, and for another program's app.
        ("g-other-app", PROGRAM, refusal(Refusal::AudienceNotGranted)),
        (
            "g-other-program",
            PROGRAM,
            refusal(Refusal::AudienceNotGranted),
        ),
        ("g-always", "codex", refusal(Refusal::AudienceNotGranted)),
        // An account that holds no API key.
        ("g-oauth", PROGRAM, fault(LauncherFault::NotAKeyAccount)),
        // A grant that is not an allowance, one nobody holds, and text that is no grant id.
        ("g-denied", PROGRAM, refusal(Refusal::UnknownGrant)),
        ("g-404", PROGRAM, refusal(Refusal::UnknownGrant)),
        (
            "not a grant",
            PROGRAM,
            "org.freedesktop.DBus.Error.InvalidArgs".to_owned(),
        ),
        // A program that is no program.
        (
            "g-always",
            "Not A Program",
            "org.freedesktop.DBus.Error.InvalidArgs".to_owned(),
        ),
    ];
    for (grant, program, expected) in cases {
        let err = launcher
            .issue(grant, program, "memfd")
            .await
            .expect_err(grant);
        assert_eq!(error_name(&err), expected, "{grant} for {program}");
    }
    // A way that is not one of the two.
    for target in ["env", "Memfd", ""] {
        let err = launcher
            .issue("g-always", PROGRAM, target)
            .await
            .expect_err("target");
        assert_eq!(
            error_name(&err),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{target:?}"
        );
    }

    // The account turned off for Llm, waiting to be signed in again, or with no key filed.
    let settings = settings(&rig).await;
    settings
        .set(
            &key("accounts.anthropic.service.llm"),
            &toml::Value::String("off".into()),
        )
        .await
        .expect("toggle");
    let err = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect_err("off");
    assert_eq!(error_name(&err), refusal(Refusal::Denied));
    settings
        .set(
            &key("accounts.anthropic.service.llm"),
            &toml::Value::String("on".into()),
        )
        .await
        .expect("toggle back");
    let keyed = AccountId::parse(KEYED).expect("id");
    assert!(
        rig.service
            .set_state(&keyed, AccountState::NeedsReauth)
            .await
    );
    let err = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect_err("reauth");
    assert_eq!(error_name(&err), refusal(Refusal::NeedsReauth));
    assert!(rig.service.set_state(&keyed, AccountState::Ok).await);
    rig.secrets.delete(&secret(KEYED)).await.expect("delete");
    let err = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect_err("no key");
    assert_eq!(error_name(&err), refusal(Refusal::NeedsReauth));

    // Nothing was handed out, so nothing was audited and no file made.
    assert!(handoffs(&rig).is_empty());
    assert!(!runtime.agent_dir().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tmpfs_way_needs_a_runtime_directory_and_the_memfd_way_does_not() {
    let rig = rig(None).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let err = launcher
        .issue("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect_err("no directory");
    assert_eq!(error_name(&err), refusal_name(Refusal::Unavailable));
    assert!(launcher.issue("g-always", PROGRAM, "memfd").await.is_ok());
    assert_eq!(handoffs(&rig).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn another_launcher_cannot_end_a_credential_it_was_not_issued() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let first = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let second = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let issued = first
        .issue("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("issued");
    let err = second
        .tokens
        .revoke_process_credential(&issued.id)
        .await
        .expect_err("not its credential");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownCredential));
    assert!(issued.path().exists());
    // A malformed id is an argument error.
    let err = second
        .tokens
        .revoke_process_credential("../etc")
        .await
        .expect_err("malformed");
    assert_eq!(error_name(&err), "org.freedesktop.DBus.Error.InvalidArgs");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_that_leaves_ends_its_credentials_and_only_its_own() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let first = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let second = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let theirs = first
        .issue("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("first's");
    // The second launcher's credential needs a grant for its own program's app.
    let codex = second
        .issue("g-other-program", "codex", "tmpfs_file")
        .await
        .expect("second's");
    let (theirs, codex) = (theirs.path(), codex.path());

    first.leave();
    eventually("the first launcher's file to go", || !theirs.exists()).await;
    assert!(codex.exists(), "the other launcher's credential stays");
    let lines = handoffs(&rig);
    let ends: Vec<_> = lines
        .iter()
        .filter_map(|e| match &e.event {
            AuditEvent::ProcessCredentialRevoked { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [CredentialEnd::LauncherGone]);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_the_grant_ends_the_credentials_under_it_and_tells_their_launcher_alone() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let mut launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let mut bystander = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let file = launcher
        .issue("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("file");
    let fd = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect("fd");
    // A credential under another grant is not touched.
    let kept = launcher
        .issue("g-second", PROGRAM, "tmpfs_file")
        .await
        .expect("kept");
    let (file_path, kept_path) = (file.path(), kept.path());

    // The person revokes it as the app holding it would (or from Settings).
    let holder = rig
        .client_as(porter_dbus::Caller {
            app: agent_app(PROGRAM),
            role: CallerRole::App,
        })
        .await;
    GrantsProxy::new(&holder)
        .await
        .expect("proxy")
        .revoke("g-always")
        .await
        .expect("revoked");

    assert!(!file_path.exists(), "the file is unlinked");
    assert!(kept_path.exists());
    // The launcher is told of each, with the reason; the memfd's copy is the launcher's to end.
    let mut told = vec![
        launcher.told(Duration::from_secs(5)).await.expect("one"),
        launcher.told(Duration::from_secs(5)).await.expect("two"),
    ];
    told.sort();
    let mut expected = vec![
        (file.id.clone(), "grant_revoked".to_owned()),
        (fd.id.clone(), "grant_revoked".to_owned()),
    ];
    expected.sort();
    assert_eq!(told, expected);
    assert_eq!(launcher.told(Duration::from_millis(300)).await, None);
    assert_eq!(
        bystander.told(Duration::from_millis(300)).await,
        None,
        "a signal goes to the launcher that was issued the credential, alone"
    );
    let reasons: Vec<_> = handoffs(&rig)
        .into_iter()
        .filter_map(|e| match e.event {
            AuditEvent::ProcessCredentialRevoked { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(reasons, [CredentialEnd::GrantRevoked; 2]);
    // They are over: the launcher can no longer end them, and a new one needs a grant.
    let err = launcher
        .tokens
        .revoke_process_credential(&file.id)
        .await
        .expect_err("already ended");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownCredential));
    let err = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect_err("revoked");
    assert_eq!(error_name(&err), refusal_name(Refusal::UnknownGrant));
    // The other grant still backs one.
    assert!(launcher.issue("g-second", PROGRAM, "memfd").await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_account_ends_its_credentials_with_that_reason() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let mut launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let issued = launcher
        .issue("g-always", PROGRAM, "tmpfs_file")
        .await
        .expect("issued");
    let path = issued.path();

    settings(&rig)
        .await
        .invoke(&key("accounts.anthropic.remove"))
        .await
        .expect("removed");

    assert!(!path.exists());
    assert_eq!(
        launcher.told(Duration::from_secs(5)).await,
        Some((issued.id.clone(), "account_removed".to_owned()))
    );
    let ends: Vec<_> = handoffs(&rig)
        .into_iter()
        .filter_map(|e| match e.event {
            AuditEvent::ProcessCredentialRevoked { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [CredentialEnd::AccountRemoved]);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_the_grant_from_settings_ends_it_too() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime)).await;
    let mut launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let issued = launcher
        .issue("g-always", PROGRAM, "memfd")
        .await
        .expect("issued");
    settings(&rig)
        .await
        .invoke(&key("accounts.anthropic.grant.g-always"))
        .await
        .expect("revoked");
    assert_eq!(
        launcher.told(Duration::from_secs(5)).await,
        Some((issued.id, "grant_revoked".to_owned()))
    );
}
