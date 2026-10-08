//! "This session only" (agent-session ask S4, lane session-scope): the agent launcher begins and
//! ends sessions (`Peer.BeginSession`, `Peer.EndSession`), asks the person for an agent program's
//! key on the consent sheet (`Peer.RequestAgentGrant`), and a grant scoped to a session goes with
//! it: the grant, the process credentials under it, the Settings row.

use crate::common;

use accountd::{AppNames, Options};
use common::host::send_input;
use common::*;
use porter_core::audit::{AuditEntry, AuditEvent, CredentialEnd, Handoff};
use porter_core::consent::{ConsentAnswer, Decision, Grant, GrantKey, GrantScope, Usage};
use porter_core::sheet::SheetInput;
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountId, AppId, AppName, AuthKind, CapabilityKind, Credential, DataClass, GrantId,
    Isolation, LauncherSession, SecretKey, SecretPurpose, SecretText, SpaceScope, UnixSeconds,
};
use porter_dbus::zvariant::{OwnedValue, Value};
use porter_dbus::{CallerRole, Details, LauncherFault, PeerProxy, Sheet, TokensProxy};
use porter_fake::llm_account;
use porter_secrets::Secrets;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use zbus::export::futures_core::Stream;

const KEY: &str = "sk-ant-api03-S3CRET-SESSION-KEY-0123456789";
const PROGRAM: &str = "claude-code";
const KEYED: &str = "anthropic";
const OAUTH: &str = "oauth-llm";
const INVALID: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// A runtime directory of its own, removed on drop.
struct Runtime(PathBuf);

impl Runtime {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "session-runtime-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("runtime dir");
        Self(dir)
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

fn session(id: &str) -> LauncherSession {
    LauncherSession::parse(id).expect("session")
}

fn always_grant() -> Grant {
    Grant {
        id: GrantId::parse("g-always").expect("id"),
        key: GrantKey {
            app: agent_app(PROGRAM),
            account: AccountId::parse(KEYED).expect("id"),
            kind: CapabilityKind::Llm,
            class: DataClass::Prompt,
            usage: Usage::Interactive,
            space: SpaceScope::Any,
        },
        decision: Decision::Allow,
        scope: GrantScope::Always,
        at: UnixSeconds(1),
    }
}

fn keyed() -> AccountId {
    AccountId::parse(KEYED).expect("id")
}

/// accountd with the two Llm accounts, the person's `Always` grant of the key account to the
/// agent, and a sheet host that says nothing until the test does.
async fn rig(runtime: Option<&Runtime>, grants: Vec<Grant>) -> Rig {
    let options = Options {
        runtime_dir: runtime.map(|r| r.0.clone()),
        app_names: AppNames::default().with_agents(&porter_provider::shipped_specs()),
        ..Options::default()
    };
    let accounts = vec![
        storage_account(),
        account(KEYED, AuthKind::ApiKey),
        account(OAUTH, AuthKind::OAuthPkce),
    ];
    let providers = vec![porter_fake::cloud_provider(), porter_fake::llm_provider()];
    let rig = Rig::start_granted(options, SheetHost::quiet(), providers, accounts, grants).await;
    rig.secrets
        .put(
            &SecretKey {
                account: keyed(),
                purpose: SecretPurpose::ApiKey,
            },
            &Credential::ApiKey(SecretText::new(KEY)),
        )
        .await
        .expect("key filed");
    rig
}

/// A fake launcher process: a connection accountd knows as `AgentLauncher`.
struct Launcher {
    connection: zbus::Connection,
    peer: PeerProxy<'static>,
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
            peer: PeerProxy::new(&connection).await.expect("proxy"),
            connection,
            tokens,
            revoked,
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

    /// `RequestAgentGrant` as the launcher does it: the Request's path, or why not.
    async fn ask(
        &self,
        program: &str,
        kind: &str,
        class: &str,
        session: &str,
    ) -> Result<(Sheet, zbus::zvariant::OwnedObjectPath), zbus::Error> {
        let sheet = Sheet::subscribe(&self.connection).await.expect("subscribe");
        let path = self
            .peer
            .request_agent_grant(program, kind, class, session, "", &sheet.options())
            .await?;
        Ok((sheet, path))
    }

    /// The sheet's `Response` for a request in `session` (empty: none), once the host is
    /// answered by `answer`.
    async fn request(&self, session: &str) -> tokio::task::JoinHandle<(u32, Details)> {
        let (mut sheet, path) = self
            .ask(PROGRAM, "llm", "prompt", session)
            .await
            .expect("sheet");
        tokio::spawn(async move { sheet.response(&path).await.expect("response") })
    }

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

    async fn issue(&self, grant: &str, target: &str) -> Result<(String, OwnedValue), zbus::Error> {
        self.tokens
            .issue_process_credential(grant, PROGRAM, target)
            .await
    }
}

/// Answers the `n`th sheet the host was opened with (counting from one).
async fn answer(rig: &Rig, n: usize, input: SheetInput) {
    eventually("the sheet host to be opened", || {
        rig.host_log.calls().opened.len() >= n
    })
    .await;
    let handle = rig.host_log.calls().opened[n - 1].0.clone();
    send_input(&rig.host_connection, &handle, &input).await;
}

fn allow(scope: GrantScope) -> SheetInput {
    SheetInput::Answer(ConsentAnswer::Allow {
        account: keyed(),
        scope,
    })
}

fn fault(fault: LauncherFault) -> String {
    fault.error_name()
}

fn path_of(value: &OwnedValue) -> PathBuf {
    match &**value {
        Value::Str(path) => PathBuf::from(path.as_str()),
        other => panic!("a path, not {other:?}"),
    }
}

fn held(rig: &Rig) -> Vec<(String, GrantScope)> {
    rig.service
        .registry()
        .grants
        .iter()
        .map(|g| (g.id.to_string(), g.scope.clone()))
        .collect()
}

fn session_lines(rig: &Rig) -> Vec<AuditEntry> {
    rig.audit
        .entries()
        .into_iter()
        .filter(|e| {
            matches!(
                e.event,
                AuditEvent::SessionGrantEnded { .. }
                    | AuditEvent::ProcessCredentialIssued { .. }
                    | AuditEvent::ProcessCredentialRevoked { .. }
            )
        })
        .collect()
}

/// Begins `sess`, asks for the key in it and answers "This session only"; the grant's id.
async fn session_grant(rig: &Rig, launcher: &Launcher, sess: &str, n: usize) -> String {
    launcher.peer.begin_session(sess).await.expect("begun");
    let response = launcher.request(sess).await;
    answer(rig, n, allow(GrantScope::Session(session(sess)))).await;
    let (code, results) = response.await.expect("joined");
    assert_eq!(code, 0, "{results:?}");
    grant_in(&results).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_belongs_to_the_connection_that_began_it_and_only_a_registered_launcher_may() {
    let rig = rig(None, vec![]).await;
    for refused in [
        rig.client_as(caller("org.example.App", CallerRole::App))
            .await,
        rig.client_as(caller("org.quire.Companion", CallerRole::Agent))
            .await,
        rig.client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
            .await,
        rig.client_as(caller("org.quire.Settings", CallerRole::Settings))
            .await,
        rig.client_as(caller("org.example.Sheets", CallerRole::SheetHost))
            .await,
        rig.stranger().await,
    ] {
        let peer = PeerProxy::new(&refused).await.expect("proxy");
        for (what, err) in [
            ("begin", peer.begin_session("sess-1").await.expect_err("b")),
            ("end", peer.end_session("sess-1").await.expect_err("e")),
            (
                "ask",
                peer.request_agent_grant(PROGRAM, "llm", "prompt", "", "", &Details::new())
                    .await
                    .expect_err("r"),
            ),
        ] {
            assert_eq!(error_name(&err), ACCESS_DENIED, "{what}");
        }
    }

    // A launcher that registered nothing has no session to begin.
    let bare = Launcher::connect(&rig, "org.example.Bare").await;
    let err = bare.peer.begin_session("sess-1").await.expect_err("bare");
    assert_eq!(error_name(&err), fault(LauncherFault::NotRegistered));

    let first = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let second = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    first.peer.begin_session("sess-1").await.expect("begun");
    // Beginning what you hold is a no-op; what another holds is taken.
    first.peer.begin_session("sess-1").await.expect("again");
    let err = second
        .peer
        .begin_session("sess-1")
        .await
        .expect_err("taken");
    assert_eq!(error_name(&err), fault(LauncherFault::SessionTaken));
    // The second cannot end it, or ask in it.
    let err = second
        .peer
        .end_session("sess-1")
        .await
        .expect_err("not its");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownSession));
    let err = second
        .ask("codex", "llm", "prompt", "sess-1")
        .await
        .map(|_| ())
        .expect_err("not its session");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownSession));
    // Bad ids are invalid, not unknown.
    for bad in ["", "Not An Id", "../x"] {
        let err = first.peer.begin_session(bad).await.expect_err(bad);
        assert_eq!(error_name(&err), INVALID, "{bad:?}");
    }

    // The owner ends it once; then it is gone, and the other may begin it.
    first.peer.end_session("sess-1").await.expect("ended");
    let err = first.peer.end_session("sess-1").await.expect_err("twice");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownSession));
    second.peer.begin_session("sess-1").await.expect("free now");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_grant_is_asked_only_for_an_open_session_of_the_launcher_and_its_own_program() {
    let rig = rig(None, vec![]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let other = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    launcher.peer.begin_session("sess-1").await.expect("begun");
    other.peer.begin_session("sess-2").await.expect("begun");

    let refused = |result: Result<(Sheet, zbus::zvariant::OwnedObjectPath), zbus::Error>| {
        error_name(&result.map(|_| ()).expect_err("refused"))
    };
    // A session never begun, ended, or another connection's.
    assert_eq!(
        refused(launcher.ask(PROGRAM, "llm", "prompt", "sess-9").await),
        fault(LauncherFault::UnknownSession)
    );
    assert_eq!(
        refused(launcher.ask(PROGRAM, "llm", "prompt", "sess-2").await),
        fault(LauncherFault::UnknownSession)
    );
    assert_eq!(
        refused(other.ask("codex", "llm", "prompt", "sess-1").await),
        fault(LauncherFault::UnknownSession)
    );
    // A program the connection did not register, a kind that is not the key's, bad words.
    assert_eq!(
        refused(launcher.ask("codex", "llm", "prompt", "sess-1").await),
        fault(LauncherFault::NotRegistered)
    );
    for (program, kind, class, session) in [
        (PROGRAM, "mail", "prompt", ""),
        (PROGRAM, "llm", "not-a-class", ""),
        (PROGRAM, "llm", "prompt", "Not An Id"),
        ("Not A Program", "llm", "prompt", ""),
    ] {
        assert_eq!(
            refused(launcher.ask(program, kind, class, session).await),
            INVALID,
            "{program} {kind} {class} {session}"
        );
    }
    // None of that opened a sheet.
    assert!(rig.host_log.calls().opened.is_empty());

    launcher.peer.end_session("sess-1").await.expect("ended");
    assert_eq!(
        refused(launcher.ask(PROGRAM, "llm", "prompt", "sess-1").await),
        fault(LauncherFault::UnknownSession)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sheet_offers_this_session_only_just_when_the_request_carries_an_open_session() {
    let rig = rig(None, vec![]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    launcher.peer.begin_session("sess-1").await.expect("begun");

    // With a session: the ask names it, and the app is the agent's, so the sheet is titled for it.
    let with = launcher.request("sess-1").await;
    eventually("the first sheet", || rig.host_log.calls().opened.len() == 1).await;
    let view = rig.host_log.calls().opened[0].2.clone();
    assert!(view.contains(r#""session":"sess-1""#), "{view}");
    assert!(view.contains("org.quire.Agent.claude-code"), "{view}");
    answer(&rig, 1, SheetInput::Dismiss).await;
    assert_eq!(with.await.expect("joined").0, 1, "dismissed");

    // Without one: no session in the ask.
    let without = launcher.request("").await;
    eventually("the second sheet", || {
        rig.host_log.calls().opened.len() == 2
    })
    .await;
    let view = rig.host_log.calls().opened[1].2.clone();
    assert!(!view.contains("session"), "{view}");
    // A sheet that answers "this session only" to an ask without one is not obeyed.
    answer(&rig, 2, allow(GrantScope::Session(session("sess-1")))).await;
    assert_eq!(without.await.expect("joined").0, 1, "dismissed");
    assert!(held(&rig).is_empty());

    // Nor a session other than the one asked in.
    let wrong = launcher.request("sess-1").await;
    answer(&rig, 3, allow(GrantScope::Session(session("sess-2")))).await;
    assert_eq!(wrong.await.expect("joined").0, 1, "dismissed");
    assert!(held(&rig).is_empty());

    // Always and Once stay open to the person in a session ask.
    let always = launcher.request("sess-1").await;
    answer(&rig, 4, allow(GrantScope::Always)).await;
    assert_eq!(always.await.expect("joined").0, 0);
    assert_eq!(
        held(&rig).into_iter().map(|(_, s)| s).collect::<Vec<_>>(),
        [GrantScope::Always]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_the_session_removes_its_grant_and_ends_the_credentials_under_it() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime), vec![always_grant()]).await;
    let mut launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let sess_grant = session_grant(&rig, &launcher, "sess-1", 1).await;
    assert_eq!(
        held(&rig),
        [
            ("g-always".to_owned(), GrantScope::Always),
            (sess_grant.clone(), GrantScope::Session(session("sess-1")))
        ]
    );

    // A credential under the session grant (a file and a descriptor) and one under the Always
    // grant, which the session does not touch.
    let (file_id, file) = launcher
        .issue(&sess_grant, "tmpfs_file")
        .await
        .expect("file");
    let (fd_id, _fd) = launcher.issue(&sess_grant, "memfd").await.expect("fd");
    let (always_id, _) = launcher.issue("g-always", "memfd").await.expect("always");
    let path = path_of(&file);
    assert_eq!(std::fs::read_to_string(&path).expect("key"), KEY);

    launcher.peer.end_session("sess-1").await.expect("ended");

    // The grant is gone and the file with it; the Always grant and its credential are not.
    assert_eq!(held(&rig), [("g-always".to_owned(), GrantScope::Always)]);
    assert!(!path.exists());
    let mut told = Vec::new();
    while let Some(one) = launcher.told(Duration::from_millis(300)).await {
        told.push(one);
    }
    told.sort();
    let mut want = vec![
        (file_id.clone(), "session_closed".to_owned()),
        (fd_id.clone(), "session_closed".to_owned()),
    ];
    want.sort();
    assert_eq!(told, want, "the launcher hears the two, and not the third");
    launcher
        .tokens
        .revoke_process_credential(&always_id)
        .await
        .expect("the always credential is still its own");
    let err = launcher
        .tokens
        .revoke_process_credential(&file_id)
        .await
        .expect_err("ended already");
    assert_eq!(error_name(&err), fault(LauncherFault::UnknownCredential));

    // The audit: the grant ended with the session, and each credential's end, as session_closed.
    let lines = session_lines(&rig);
    let ended: Vec<_> = lines
        .iter()
        .filter_map(|e| match &e.event {
            AuditEvent::SessionGrantEnded { grant, session } => {
                Some((grant.to_string(), session.to_string(), e.app.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        ended,
        [(sess_grant, "sess-1".to_owned(), Some(agent_app(PROGRAM)))]
    );
    let closed = lines
        .iter()
        .filter(|e| {
            matches!(
                e.event,
                AuditEvent::ProcessCredentialRevoked {
                    reason: CredentialEnd::SessionClosed,
                    ..
                }
            )
        })
        .count();
    assert_eq!(closed, 2, "{lines:?}");
    // The credentials ended before the grant went: none is reported as a withdrawn grant.
    assert!(!lines.iter().any(|e| matches!(
        e.event,
        AuditEvent::ProcessCredentialRevoked {
            reason: CredentialEnd::GrantRevoked,
            ..
        }
    )));
    let issued = lines
        .iter()
        .filter(|e| {
            matches!(
                e.event,
                AuditEvent::ProcessCredentialIssued {
                    handoff: Handoff::TmpfsFile | Handoff::Memfd,
                    ..
                }
            )
        })
        .count();
    assert_eq!(issued, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launcher_that_leaves_closes_its_sessions_and_nobody_else_hears_of_it() {
    let runtime = Runtime::new();
    let rig = rig(Some(&runtime), vec![always_grant()]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let mut bystander = Launcher::registered(&rig, "org.example.Codex", &["codex"]).await;
    let sess_grant = session_grant(&rig, &launcher, "sess-1", 1).await;
    let (_, file) = launcher
        .issue(&sess_grant, "tmpfs_file")
        .await
        .expect("file");
    let (_, _fd) = launcher.issue("g-always", "memfd").await.expect("always");
    let path = path_of(&file);
    assert!(path.exists());

    drop(launcher);
    eventually("the session grant to go with its launcher", || {
        held(&rig) == [("g-always".to_owned(), GrantScope::Always)]
    })
    .await;
    eventually("the file to be unlinked", || !path.exists()).await;

    // Session credentials end as session_closed; the launcher's other credential as
    // launcher_gone; the grant's end is audited.
    eventually("the audit", || session_lines(&rig).len() >= 5).await;
    let reasons: Vec<CredentialEnd> = session_lines(&rig)
        .iter()
        .filter_map(|e| match e.event {
            AuditEvent::ProcessCredentialRevoked { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        {
            let mut r = reasons.clone();
            r.sort_by_key(|r| r.word());
            r
        },
        [CredentialEnd::LauncherGone, CredentialEnd::SessionClosed]
    );
    assert!(session_lines(&rig).iter().any(|e| matches!(
        &e.event,
        AuditEvent::SessionGrantEnded { session, .. } if session.as_str() == "sess-1"
    )));
    // The signal is unicast: a launcher that did not hold the credentials hears nothing.
    assert_eq!(bystander.told(Duration::from_millis(300)).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_closes_while_its_sheet_is_open_leaves_no_grant() {
    let rig = rig(None, vec![]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    launcher.peer.begin_session("sess-1").await.expect("begun");
    let response = launcher.request("sess-1").await;
    eventually("the sheet", || rig.host_log.calls().opened.len() == 1).await;

    launcher.peer.end_session("sess-1").await.expect("ended");
    answer(&rig, 1, allow(GrantScope::Session(session("sess-1")))).await;
    let (code, _) = response.await.expect("joined");
    assert_eq!(code, 1, "the request ends dismissed");
    assert!(held(&rig).is_empty(), "{:?}", held(&rig));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_credential_takes_an_open_session_grant_and_still_refuses_a_once_grant() {
    let runtime = Runtime::new();
    let once = Grant {
        id: GrantId::parse("g-once").expect("id"),
        scope: GrantScope::Once,
        ..always_grant()
    };
    let rig = rig(Some(&runtime), vec![always_grant(), once]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let sess_grant = session_grant(&rig, &launcher, "sess-1", 1).await;

    launcher.issue(&sess_grant, "memfd").await.expect("session");
    launcher.issue("g-always", "memfd").await.expect("always");
    let err = launcher.issue("g-once", "memfd").await.expect_err("once");
    assert_eq!(error_name(&err), fault(LauncherFault::OnceGrant));
    // After the session the grant is gone altogether.
    launcher.peer.end_session("sess-1").await.expect("ended");
    let err = launcher
        .issue(&sess_grant, "memfd")
        .await
        .expect_err("gone");
    assert_eq!(error_name(&err), refusal_name(Refusal::UnknownGrant));
}

#[tokio::test(flavor = "multi_thread")]
async fn inferds_verdict_reads_a_session_grant_as_allowed_while_it_is_open_and_not_after() {
    let rig = rig(None, vec![]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let daemon = rig
        .client_as(caller("org.quire.Inference", CallerRole::PorterDaemon))
        .await;
    let peer = PeerProxy::new(&daemon).await.expect("proxy");
    let need = porter_dbus::need_to_dbus(&porter_core::Need::Llm(porter_core::need::LlmNeed {
        features: std::collections::BTreeSet::from([porter_core::capability::LlmFeature::Chat]),
        context: porter_core::Tokens(0),
    }));
    let app = (
        "org.quire.Agent.claude-code".to_owned(),
        "unsandboxed".to_owned(),
    );
    let verdict = || async {
        let rows = peer
            .verdicts(&app, &need, "prompt", "interactive")
            .await
            .expect("verdicts");
        let row = rows
            .into_iter()
            .find(|(account, ..)| account == KEYED)
            .expect("the key account is a row");
        (
            row.1.clone(),
            text_of(&row.2, "scope"),
            text_of(&row.2, "session"),
        )
    };

    assert_eq!(verdict().await, ("ask".to_owned(), None, None));
    let grant = session_grant(&rig, &launcher, "sess-1", 1).await;
    assert_eq!(
        verdict().await,
        (
            "granted".to_owned(),
            Some("session".to_owned()),
            Some("sess-1".to_owned())
        )
    );
    let row = peer
        .verdicts(&app, &need, "prompt", "interactive")
        .await
        .expect("verdicts")
        .into_iter()
        .find(|(account, ..)| account == KEYED)
        .expect("row");
    assert_eq!(text_of(&row.2, "grant"), Some(grant));

    launcher.peer.end_session("sess-1").await.expect("ended");
    assert_eq!(verdict().await, ("ask".to_owned(), None, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_shows_a_session_grant_for_this_session_and_the_row_goes_with_it() {
    let rig = rig(None, vec![]).await;
    let launcher = Launcher::registered(&rig, "org.quire.AgentLauncher", &[PROGRAM]).await;
    let grant = session_grant(&rig, &launcher, "sess-1", 1).await;
    let row = format!("accounts.{KEYED}.grant.{grant}");
    let label = async |rig: &Rig| {
        let schema = settings(rig).await.describe().await.expect("schema");
        schema
            .key
            .iter()
            .find(|k| k.path.0 == row)
            .map(|k| k.label.0.clone())
    };
    assert_eq!(
        label(&rig).await.as_deref(),
        Some("Claude Code can use Language model (this session)")
    );
    launcher.peer.end_session("sess-1").await.expect("ended");
    assert_eq!(label(&rig).await, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_grant_in_the_store_does_not_survive_a_restart() {
    let mut stale = always_grant();
    stale.id = GrantId::parse("g-stale").expect("id");
    stale.scope = GrantScope::Session(session("sess-from-last-run"));
    let rig = rig(None, vec![always_grant(), stale]).await;
    assert_eq!(held(&rig), [("g-always".to_owned(), GrantScope::Always)]);
    assert!(rig.audit.entries().iter().any(|e| matches!(
        &e.event,
        AuditEvent::SessionGrantEnded { grant, .. } if grant.as_str() == "g-stale"
    )));
    // Saved: the store no longer holds it either.
    let stored = rig.store.stored().expect("saved");
    assert_eq!(stored.grants.len(), 1);
}
