//! `org.quire.Accounts1.Peer` at `/org/quire/Accounts1`: what the other daemons ask accountd on
//! behalf of an app (porter PLAN G3). Callable only by a connection whose caller role is
//! `PorterDaemon` (inferd, syncd); accountd refuses every other sender `AccessDenied`. The one
//! exception is the launcher's three (`SetAgentState`, `RegisterLauncher`, `ReportAgentLogin`,
//! `ReportAgentLogout`), which only `AgentLauncher` may call (and `PorterDaemon` may
//! not); accountd answers it with two unicast signals, `AgentLoginRequested` and
//! `AgentLogoutRequested`, sent to the connection that registered the agent's program. The app
//! is named by the calling daemon from its own connection, never by the app. There is no
//! `OpenCredential`: syncd opens an authenticated stream like any app (`Tokens`).
//!
//! The launcher also owns launcher sessions (lane session-scope): `BeginSession` and
//! `EndSession` name the span a "This session only" grant lasts, and `RequestAgentGrant` opens
//! the consent sheet for an agent program's key, offering that scope when the request names an
//! open session of the caller.

use crate::args::{AppArg, Details, NeedArg, VerdictArg};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedFd;
use zbus::zvariant::OwnedObjectPath;

/// The daemon's side of the conversation.
#[zbus::proxy(
    interface = "org.quire.Accounts1.Peer",
    default_service = "org.quire.Accounts1",
    default_path = "/org/quire/Accounts1"
)]
pub trait Peer {
    /// What the consent store says for `app` on each account whose offer fits `need`, so inferd
    /// can route to a cloud account only when the app holds a grant. Reveals no secret.
    fn verdicts(
        &self,
        app: &AppArg,
        need: &NeedArg,
        class: &str,
        usage: &str,
    ) -> zbus::Result<Vec<VerdictArg>>;
    /// The API key of a granted account, on a sealed memfd and never a string on the bus.
    fn resolve_key(&self, grant: &str) -> zbus::Result<OwnedFd>;
    /// Reports a probed local runtime as an account of `provider` with its models as claims
    /// (kind slug and the claim's fields by name, as `Account.Capabilities`), in `state`
    /// (`ok`, `offline`); returns the account id.
    fn report_local(
        &self,
        provider: &str,
        claims: Vec<(String, Details)>,
        state: &str,
    ) -> zbus::Result<String>;
    /// What an agent program says of its own sign-in (`ready`, `needs_login`), for the account
    /// of an agent that signs itself in (`AuthKind::AgentLogin`). Only the launcher
    /// (`CallerRole::AgentLauncher`) may say it; porter holds no more of an agent's login.
    fn set_agent_state(&self, account: &str, state: &str) -> zbus::Result<()>;
    /// Registers the connection as the launcher of these agent programs (`AgentProgram` ids).
    /// `AgentLauncher` only. The registration lives as long as the connection. One launcher per
    /// program, first wins: a program a live connection already holds is `AlreadyRegistered`
    /// (`LauncherFault`); registering a program the caller holds is a no-op, and later
    /// calls add more programs.
    fn register_launcher(&self, programs: &[&str]) -> zbus::Result<()>;
    /// Opens a launcher session (a `LauncherSession` id the launcher chose, usually an ACP
    /// session). `AgentLauncher` only, and only a connection that registered a program. The
    /// session belongs to this connection and ends at `EndSession` or when the connection leaves
    /// the bus. A session another connection holds is `SessionTaken`; beginning one this
    /// connection holds is a no-op.
    fn begin_session(&self, session: &str) -> zbus::Result<()>;
    /// Ends a session this connection began: its session grants are removed and the process
    /// credentials under them end (`session_closed`). A session that is not open, or is
    /// another connection's, is `UnknownSession`.
    fn end_session(&self, session: &str) -> zbus::Result<()>;
    /// Opens the consent sheet for an agent program's key: the app is `org.quire.Agent.<program>`
    /// and `kind` must be `llm`. `session` is empty, or an open session of this connection, in
    /// which case the sheet also offers "This session only" (`GrantScope::Session`). Returns a
    /// Request object, answered as `Manager.Choose`'s is (the chosen candidate and its grant).
    /// `AgentLauncher` only, for a program this connection registered.
    fn request_agent_grant(
        &self,
        program: &str,
        kind: &str,
        class: &str,
        session: &str,
        parent_window: &str,
        options: &Details,
    ) -> zbus::Result<OwnedObjectPath>;
    /// What the launcher reports of an `AgentLoginRequested`: `outcome` is `ready`, `failed` or
    /// `cancelled`, `reason` one of `LoginFault`'s words for `failed` and empty otherwise (see
    /// `LoginOutcome::to_wire`). Only the connection that got the request may answer it; any
    /// other is `UnknownRequest`. Nothing else crosses: no token, no URL, no code.
    fn report_agent_login(&self, request: &str, outcome: &str, reason: &str) -> zbus::Result<()>;
    /// What the launcher reports of an `AgentLogoutRequested`; `ready` means signed out.
    fn report_agent_logout(&self, request: &str, outcome: &str, reason: &str) -> zbus::Result<()>;
    /// Sent to the registrant of `program` alone: sign the agent in (the agent does it itself;
    /// accountd learns only the outcome).
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
}

/// accountd's side.
#[derive(Debug, Default)]
pub struct PeerSkeleton;

#[zbus::interface(name = "org.quire.Accounts1.Peer")]
impl PeerSkeleton {
    fn verdicts(
        &self,
        app: AppArg,
        need: NeedArg,
        class: String,
        usage: String,
    ) -> fdo::Result<Vec<VerdictArg>> {
        let _ = (app, need, class, usage);
        Err(crate::introspect::frozen())
    }

    fn resolve_key(&self, grant: String) -> fdo::Result<OwnedFd> {
        let _ = grant;
        Err(crate::introspect::frozen())
    }

    fn report_local(
        &self,
        provider: String,
        claims: Vec<(String, Details)>,
        state: String,
    ) -> fdo::Result<String> {
        let _ = (provider, claims, state);
        Err(crate::introspect::frozen())
    }

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

    fn request_agent_grant(
        &self,
        program: String,
        kind: String,
        class: String,
        session: String,
        parent_window: String,
        options: Details,
    ) -> fdo::Result<OwnedObjectPath> {
        let _ = (program, kind, class, session, parent_window, options);
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
}
