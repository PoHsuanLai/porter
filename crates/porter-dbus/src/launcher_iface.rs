//! `org.quire.Inference1.Launcher` at `/org/quire/Inference1` on inferd's name: what the agent
//! launcher (docket-acp) asks of the daemon that holds the AI keys. Callable only by a connection
//! whose caller role is `AgentLauncher` (a unit's row in inferd's caller table); inferd refuses
//! every other sender `AccessDenied`.
//!
//! The launcher registers the agent programs it runs (`RegisterLauncher`); inferd answers with two
//! unicast signals, `AgentLoginRequested` and `AgentLogoutRequested`, sent to the connection that
//! registered the agent's program, and the launcher reports only a coarse outcome
//! (`ReportAgentLogin`, `ReportAgentLogout`). Nothing of an agent's login crosses: no token, no
//! URL, no code.
//!
//! The launcher also owns launcher sessions: `BeginSession` and `EndSession` name the span a
//! "This session only" permit lasts, and `RequestAgentKey` opens the consent sheet for an agent
//! program's key, offering that scope when the request names an open session of the caller. For an
//! agent that cannot be pointed at inferd, `IssueProcessCredential` hands the process one key, on a
//! sealed memfd or in a 0600 file, under a permit; `ProcessCredentialRevoked` tells the launcher
//! that inferd ended it.
//!
//! Errors are `org.quire.Inference1.Error.<Name>`: the refusals of [`crate::refusal`] and the
//! launcher's own, [`crate::LauncherFault`].

use crate::args::Details;
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// The launcher's side of the conversation.
#[zbus::proxy(
    interface = "org.quire.Inference1.Launcher",
    default_service = "org.quire.Inference1",
    default_path = "/org/quire/Inference1"
)]
pub trait Launcher {
    /// What an agent program says of its own sign-in (`ready`, `needs_login`), for the account
    /// of an agent that signs itself in. Only the launcher may say it; porter holds no more of an
    /// agent's login.
    fn set_agent_state(&self, account: &str, state: &str) -> zbus::Result<()>;
    /// Registers the connection as the launcher of these agent programs (`AgentProgram` ids). The
    /// registration lives as long as the connection. One launcher per program, first wins: a
    /// program a live connection already holds is `AlreadyRegistered` (`LauncherFault`);
    /// registering a program the caller holds is a no-op, and later calls add more programs.
    fn register_launcher(&self, programs: &[&str]) -> zbus::Result<()>;
    /// Opens a launcher session (a `LauncherSession` id the launcher chose, usually an ACP
    /// session). Only a connection that registered a program may. The session belongs to this
    /// connection and ends at `EndSession` or when the connection leaves the bus. A session
    /// another connection holds is `SessionTaken`; beginning one this connection holds is a
    /// no-op.
    fn begin_session(&self, session: &str) -> zbus::Result<()>;
    /// Ends a session this connection began: its session permits are removed and the process
    /// credentials under them end (`session_closed`). A session that is not open, or is another
    /// connection's, is `UnknownSession`.
    fn end_session(&self, session: &str) -> zbus::Result<()>;
    /// Opens the consent sheet for an agent program's key: the app is `org.quire.Agent.<program>`.
    /// `session` is empty, or an open session of this connection, in which case the sheet also
    /// offers "This session only". Returns a Request object, answered with the chosen key and
    /// the permit for it: the results of a `Choose` response, whose `grant` is the permit's id.
    /// For a program this connection registered.
    fn request_agent_key(
        &self,
        program: &str,
        class: &str,
        session: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
    /// What the launcher reports of an `AgentLoginRequested`: `outcome` is `ready`, `failed` or
    /// `cancelled`, `reason` one of `LoginFault`'s words for `failed` and empty otherwise (see
    /// `LoginOutcome::to_wire`). Only the connection that got the request may answer it; any
    /// other is `UnknownRequest`.
    fn report_agent_login(&self, request: &str, outcome: &str, reason: &str) -> zbus::Result<()>;
    /// What the launcher reports of an `AgentLogoutRequested`; `ready` means signed out.
    fn report_agent_logout(&self, request: &str, outcome: &str, reason: &str) -> zbus::Result<()>;
    /// An API key for a process the launcher spawns: for a program the caller's connection has
    /// registered, under `permit` (a key permit for `org.quire.Agent.<program>`). `target` is
    /// `memfd` or `tmpfs_file` (`Handoff`). The reply is the credential's id and its handle in a
    /// variant: a file descriptor (`h`, a sealed memfd) for `memfd`, the path (`s`) of a 0600
    /// file for `tmpfs_file`. The key is in neither the body nor any signal.
    fn issue_process_credential(
        &self,
        permit: &str,
        program: &str,
        target: &str,
    ) -> zbus::Result<(String, OwnedValue)>;
    /// Ends a credential this connection was issued: a tmpfs file is unlinked, and the launcher
    /// ends the process (a memfd that was passed on cannot be recalled).
    fn revoke_process_credential(&self, id: &str) -> zbus::Result<()>;
    /// Sent to the registrant of `program` alone: sign the agent in (the agent does it itself;
    /// inferd learns only the outcome).
    #[zbus(signal)]
    fn agent_login_requested(
        &self,
        request: String,
        account: String,
        program: String,
    ) -> zbus::Result<()>;
    /// Sent to the registrant of `program` alone: sign the agent out.
    #[zbus(signal)]
    fn agent_logout_requested(
        &self,
        request: String,
        account: String,
        program: String,
    ) -> zbus::Result<()>;
    /// Sent to the launcher that was issued the credential, alone: inferd ended it (the permit
    /// was revoked or the account removed), so the launcher must end the process. `reason` is a
    /// `CredentialEnd` word. Not sent when the launcher itself ended it or left the bus.
    #[zbus(signal)]
    fn process_credential_revoked(&self, id: String, reason: String) -> zbus::Result<()>;
}

/// inferd's side: the interface's shape, every method answering `NotSupported`.
#[derive(Debug, Default)]
pub struct LauncherSkeleton;

#[zbus::interface(name = "org.quire.Inference1.Launcher")]
impl LauncherSkeleton {
    fn set_agent_state(&self, account: String, state: String) -> fdo::Result<()> {
        let _ = (account, state);
        Err(crate::introspect::frozen())
    }

    fn register_launcher(&self, programs: Vec<String>) -> fdo::Result<()> {
        let _ = programs;
        Err(crate::introspect::frozen())
    }

    fn begin_session(&self, session: String) -> fdo::Result<()> {
        let _ = session;
        Err(crate::introspect::frozen())
    }

    fn end_session(&self, session: String) -> fdo::Result<()> {
        let _ = session;
        Err(crate::introspect::frozen())
    }

    fn request_agent_key(
        &self,
        program: String,
        class: String,
        session: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (program, class, session, parent_window, options);
        Err(crate::introspect::frozen())
    }

    fn report_agent_login(
        &self,
        request: String,
        outcome: String,
        reason: String,
    ) -> fdo::Result<()> {
        let _ = (request, outcome, reason);
        Err(crate::introspect::frozen())
    }

    fn report_agent_logout(
        &self,
        request: String,
        outcome: String,
        reason: String,
    ) -> fdo::Result<()> {
        let _ = (request, outcome, reason);
        Err(crate::introspect::frozen())
    }

    fn issue_process_credential(
        &self,
        permit: String,
        program: String,
        target: String,
    ) -> fdo::Result<(String, OwnedValue)> {
        let _ = (permit, program, target);
        Err(crate::introspect::frozen())
    }

    fn revoke_process_credential(&self, id: String) -> fdo::Result<()> {
        let _ = id;
        Err(crate::introspect::frozen())
    }

    #[zbus(signal)]
    async fn agent_login_requested(
        emitter: &SignalEmitter<'_>,
        request: &str,
        account: &str,
        program: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn agent_logout_requested(
        emitter: &SignalEmitter<'_>,
        request: &str,
        account: &str,
        program: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn process_credential_revoked(
        emitter: &SignalEmitter<'_>,
        id: &str,
        reason: &str,
    ) -> zbus::Result<()>;
}
