//! The launcher's side of an agent's login (lane agent-login; design/31 R7-R9, C5, C6). The agent
//! launcher (docket-acp) registers the programs it runs, hears when accountd asks it to sign an
//! agent in or out, has the agent do it, and reports only a coarse [`LoginOutcome`]. Nothing of
//! the login itself (a token, a URL, a code) crosses to porter: there is no way to send one.
//!
//! For an agent that cannot be pointed at inferd the launcher can also be handed an API key for
//! the process it spawns ([`Launcher::issue_credential`], lane p2-handoff; the duties and the
//! residual risk are in [`crate::credential`]).
//!
//! The caller must be known to accountd as the agent launcher (`CallerRole::AgentLauncher`, a row
//! of `callers.toml` a machine writes); any other caller is [`LauncherError::Denied`].
//!
//! ```ignore
//! let launcher = Launcher::connect(&connection).await?;
//! let mut requests = launcher.requests().await?;       // subscribe first
//! launcher.register(&[AgentProgram::parse("claude-code")?]).await?;
//! while let Some(ask) = requests.next().await {
//!     let ask = ask?;
//!     let outcome = run_the_agents_own_login(&ask).await;   // the agent's, never porter's
//!     match ask.kind {
//!         AskKind::Login => launcher.report_login(&ask.request, outcome).await?,
//!         AskKind::Logout => launcher.report_logout(&ask.request, outcome).await?,
//!     }
//! }
//! ```

use crate::credential::{CredentialHandle, ProcessCredential, Revocations};
use porter_core::audit::Handoff;
use porter_core::capability::AgentProgram;
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountsReply, Candidate, CoreError, DataClass, GrantId, LauncherSession,
    LoginOutcome, LoginRequestId, ProcessCredentialId,
};
use porter_dbus::{
    AgentLoginRequested, AgentLoginRequestedStream, AgentLogoutRequested,
    AgentLogoutRequestedStream, BusConnection, BusError, BusFailure, BusStream, Closer,
    LauncherFault, PeerProxy, Sheet, SheetError, SheetKind, TokensProxy, classify, is_invalid_args,
    launcher_fault_of, refusal_of, reply_of, zvariant::Value,
};
use std::os::fd::AsFd;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Why a launcher call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LauncherError {
    /// A live connection already launches one of the programs (first wins).
    #[error("another launcher already holds a program")]
    AlreadyRegistered,
    /// No such request for this connection: it was never made, was answered already, expired, or
    /// was sent to another connection.
    #[error("no such request for this connection")]
    UnknownRequest,
    /// `issue_credential` for a program this connection has not registered.
    #[error("this connection has not registered that program")]
    NotRegistered,
    /// `issue_credential` under a `Once` grant: it is spent by its first use, so it cannot back a
    /// key a process holds for its life. Ask the person for an always grant.
    #[error("a once grant cannot back a process credential")]
    OnceGrant,
    /// `issue_credential` for an account that holds no API key (an OAuth account, an agent that
    /// signs itself in, a local runtime).
    #[error("the account holds no API key to hand over")]
    NotAKeyAccount,
    /// `revoke_credential` for a credential this connection was not issued: never made, ended
    /// already (accountd may have ended it: see [`crate::Revocations`]), or another's.
    #[error("no such credential for this connection")]
    UnknownCredential,
    /// `begin_session` for a session id another live connection holds.
    #[error("another launcher holds that session")]
    SessionTaken,
    /// `end_session`, `request_agent_grant` or `issue_credential` for a session that is not open
    /// for this connection: never begun, ended already, or another connection's.
    #[error("no such open session for this connection")]
    UnknownSession,
    /// accountd refused the grant or the account for a reason an app also meets: `UnknownGrant`,
    /// `AudienceNotGranted` (the grant is not for `org.quire.Agent.<program>` or not the Llm
    /// kind), `NeedsReauth`, `Denied` (the person turned the account off), `Unavailable`.
    #[error("refused: {0:?}")]
    Refused(Refusal),
    /// accountd does not know this caller as the agent launcher.
    #[error("refused by accountd: {0}")]
    Denied(String),
    /// accountd refused an argument (a program or an id that is not one).
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// accountd is not there.
    #[error("no account service reachable")]
    Unreachable,
    /// accountd sent something that is not porter's protocol.
    #[error("malformed: {0}")]
    Malformed(String),
}

impl From<BusError> for LauncherError {
    fn from(error: BusError) -> Self {
        match (launcher_fault_of(&error), refusal_of(&error)) {
            (Some(LauncherFault::AlreadyRegistered), _) => LauncherError::AlreadyRegistered,
            (Some(LauncherFault::UnknownRequest), _) => LauncherError::UnknownRequest,
            (Some(LauncherFault::NotRegistered), _) => LauncherError::NotRegistered,
            (Some(LauncherFault::OnceGrant), _) => LauncherError::OnceGrant,
            (Some(LauncherFault::NotAKeyAccount), _) => LauncherError::NotAKeyAccount,
            (Some(LauncherFault::UnknownCredential), _) => LauncherError::UnknownCredential,
            (Some(LauncherFault::SessionTaken), _) => LauncherError::SessionTaken,
            (Some(LauncherFault::UnknownSession), _) => LauncherError::UnknownSession,
            (None, Some(refusal)) => LauncherError::Refused(refusal),
            (None, None) if is_invalid_args(&error) => LauncherError::Invalid(error.to_string()),
            (None, None) => match classify(&error) {
                BusFailure::NoDaemon => LauncherError::Unreachable,
                BusFailure::Denied(why) => LauncherError::Denied(why),
                BusFailure::Other(why) => LauncherError::Malformed(why),
            },
        }
    }
}

impl From<CoreError> for LauncherError {
    fn from(error: CoreError) -> Self {
        LauncherError::Malformed(error.to_string())
    }
}

/// What accountd asks of the launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    /// Have the agent sign itself in.
    Login,
    /// Have the agent sign itself out.
    Logout,
}

/// One request accountd sent to this launcher alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherRequest {
    /// In or out.
    pub kind: AskKind,
    /// The id to report the outcome under.
    pub request: LoginRequestId,
    /// The agent account the request is for.
    pub account: AccountId,
    /// The program (one the launcher registered) that is to sign in or out.
    pub program: AgentProgram,
}

/// The launcher's calls to accountd.
#[derive(Debug, Clone)]
pub struct Launcher {
    connection: BusConnection,
    peer: PeerProxy<'static>,
    tokens: TokensProxy<'static>,
}

/// A data class as its slug, the form the bus takes.
fn slug(class: DataClass) -> Result<String, LauncherError> {
    match serde_json::to_value(class) {
        Ok(serde_json::Value::String(slug)) => Ok(slug),
        _ => Err(LauncherError::Malformed("a data class with no slug".into())),
    }
}

/// Closes a sheet whose wait was dropped, so an abandoned request leaves none on the screen.
struct Abandon(Option<Closer>);

impl Drop for Abandon {
    fn drop(&mut self) {
        let Some(closer) = self.0.take() else { return };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(closer.close());
        }
    }
}

impl Launcher {
    /// A launcher over `connection`, which accountd knows as the agent launcher.
    pub async fn connect(connection: &BusConnection) -> Result<Self, LauncherError> {
        Ok(Self {
            connection: connection.clone(),
            peer: PeerProxy::new(connection).await?,
            tokens: TokensProxy::new(connection).await?,
        })
    }

    /// Opens `session`, the span a grant "for this session only" lasts (the launcher names it,
    /// usually from an ACP session id). It belongs to this connection, which must have
    /// registered a program, and ends at [`Launcher::end_session`] or when the connection
    /// leaves the bus. A session another connection holds is [`LauncherError::SessionTaken`];
    /// beginning one this connection holds is a no-op.
    pub async fn begin_session(&self, session: &LauncherSession) -> Result<(), LauncherError> {
        Ok(self.peer.begin_session(session.as_str()).await?)
    }

    /// Ends a session this connection began: accountd removes the grants scoped to it and ends
    /// the process credentials under them (`CredentialEnd::SessionClosed`), telling this
    /// connection of each in [`Launcher::revocations`] so the launcher can end the processes. A
    /// session that is not open for this connection is [`LauncherError::UnknownSession`].
    pub async fn end_session(&self, session: &LauncherSession) -> Result<(), LauncherError> {
        Ok(self.peer.end_session(session.as_str()).await?)
    }

    /// Asks the person, on accountd's consent sheet titled for `program`, which of their
    /// accounts the agent may use for its key (the Llm kind, interactive use). With `session`,
    /// an open session of this connection, the sheet also offers "This session only", and a
    /// grant made so is scoped to it. Returns the chosen account and its grant; a person who
    /// closes the sheet or refuses is [`LauncherError::Refused`] (`Dismissed`, `Denied`). The
    /// program must be one this connection registered.
    pub async fn request_agent_grant(
        &self,
        program: &AgentProgram,
        class: DataClass,
        session: Option<&LauncherSession>,
    ) -> Result<Candidate, LauncherError> {
        let mut listener = Sheet::subscribe(&self.connection).await?;
        // Armed before the call: a dropped call may already have made the sheet.
        let mut abandon = Abandon(listener.closer(None));
        let path = self
            .peer
            .request_agent_grant(
                program.as_str(),
                "llm",
                &slug(class)?,
                session.map_or("", LauncherSession::as_str),
                "",
                &listener.options(),
            )
            .await;
        let path = match path {
            Ok(path) => path,
            Err(error) => {
                abandon.0 = None;
                return Err(error.into());
            }
        };
        abandon.0 = listener.closer(Some(path.clone()));
        let (code, results) = listener
            .response(&path)
            .await
            .map_err(|error| match error {
                SheetError::Bus(error) => LauncherError::from(error),
                SheetError::Gone => LauncherError::Unreachable,
                SheetError::Unreadable => {
                    LauncherError::Malformed("a Response that is not (u, a{sv})".into())
                }
            })?;
        abandon.0 = None;
        match reply_of(SheetKind::Choose, code, results)? {
            AccountsReply::Chosen(candidate) => Ok(candidate),
            AccountsReply::Refused(refusal) => Err(LauncherError::Refused(refusal)),
            other => Err(LauncherError::Malformed(format!(
                "a chooser answered with {other:?}"
            ))),
        }
    }

    /// An API key for a process this launcher is about to spawn for `program`, under `grant`
    /// (the person's grant of an account to the app `org.quire.Agent.<program>`, kind Llm, scope
    /// always, or "this session only" for a session this connection holds open), handed over by
    /// `target`: a sealed memfd to pass to the child, or a 0600 file under `$XDG_RUNTIME_DIR`.
    /// The program must be one this connection registered. A credential under a session grant
    /// ends when the session does ([`Launcher::end_session`], or this connection leaving).
    ///
    /// For an agent that cannot be pointed at inferd (P4, the metered route, needs none of this).
    /// The key is outside porter's meter and the child can read its own key: see the module
    /// documentation of [`crate::credential`] for the launcher's duties.
    pub async fn issue_credential(
        &self,
        grant: &GrantId,
        program: &AgentProgram,
        target: Handoff,
    ) -> Result<ProcessCredential, LauncherError> {
        let (id, handle) = self
            .tokens
            .issue_process_credential(grant.as_str(), program.as_str(), target.word())
            .await?;
        let id = ProcessCredentialId::parse(&id)?;
        let handle = match (target, &*handle) {
            (Handoff::Memfd, Value::Fd(fd)) => CredentialHandle::Memfd(
                fd.as_fd()
                    .try_clone_to_owned()
                    .map_err(|why| LauncherError::Malformed(why.to_string()))?,
            ),
            (Handoff::TmpfsFile, Value::Str(path)) => {
                CredentialHandle::TmpfsFile(path.as_str().into())
            }
            _ => {
                return Err(LauncherError::Malformed(
                    "the handle is not the kind asked for".to_owned(),
                ));
            }
        };
        Ok(ProcessCredential::new(id, handle))
    }

    /// Ends a credential this connection was issued, because the process that held it exited or
    /// was killed: a tmpfs file is unlinked and accountd forgets it. Not for a credential
    /// accountd ended itself (that is [`LauncherError::UnknownCredential`]).
    pub async fn revoke_credential(&self, id: &ProcessCredentialId) -> Result<(), LauncherError> {
        Ok(self.tokens.revoke_process_credential(id.as_str()).await?)
    }

    /// What accountd tells this connection when it ends a credential itself (the grant was
    /// revoked, the account removed): the launcher must end that process. Subscribe before
    /// issuing, so none is missed.
    pub async fn revocations(&self) -> Result<Revocations, LauncherError> {
        Ok(Revocations::new(
            self.tokens.receive_process_credential_revoked().await?,
        ))
    }

    /// Registers the programs this connection launches, for as long as it lives. A program
    /// another live connection holds is [`LauncherError::AlreadyRegistered`] and none of the list
    /// is taken; registering a program this connection holds is a no-op, and a later call adds
    /// more.
    pub async fn register(&self, programs: &[AgentProgram]) -> Result<(), LauncherError> {
        let names: Vec<&str> = programs.iter().map(AgentProgram::as_str).collect();
        Ok(self.peer.register_launcher(&names).await?)
    }

    /// The requests accountd sends this connection. Subscribe before registering, so none is
    /// missed.
    pub async fn requests(&self) -> Result<Requests, LauncherError> {
        Ok(Requests {
            logins: self.peer.receive_agent_login_requested().await?,
            logouts: self.peer.receive_agent_logout_requested().await?,
        })
    }

    /// Reports what became of a login request. Only the connection that was sent it may.
    pub async fn report_login(
        &self,
        request: &LoginRequestId,
        outcome: LoginOutcome,
    ) -> Result<(), LauncherError> {
        let (word, reason) = outcome.to_wire();
        Ok(self
            .peer
            .report_agent_login(request.as_str(), word, reason)
            .await?)
    }

    /// Reports what became of a logout request; `Ready` means the agent is signed out.
    pub async fn report_logout(
        &self,
        request: &LoginRequestId,
        outcome: LoginOutcome,
    ) -> Result<(), LauncherError> {
        let (word, reason) = outcome.to_wire();
        Ok(self
            .peer
            .report_agent_logout(request.as_str(), word, reason)
            .await?)
    }
}

/// The stream of [`LauncherRequest`]s; an item is an error when accountd sent one that is not
/// well formed.
#[derive(Debug)]
pub struct Requests {
    logins: AgentLoginRequestedStream,
    logouts: AgentLogoutRequestedStream,
}

fn request(
    kind: AskKind,
    request: &str,
    account: &str,
    program: &str,
) -> Result<LauncherRequest, LauncherError> {
    Ok(LauncherRequest {
        kind,
        request: LoginRequestId::parse(request)?,
        account: AccountId::parse(account)?,
        program: AgentProgram::parse(program)?,
    })
}

impl Requests {
    fn login(signal: AgentLoginRequested) -> Result<LauncherRequest, LauncherError> {
        let args = signal.args()?;
        request(AskKind::Login, &args.request, &args.account, &args.program)
    }

    fn logout(signal: AgentLogoutRequested) -> Result<LauncherRequest, LauncherError> {
        let args = signal.args()?;
        request(AskKind::Logout, &args.request, &args.account, &args.program)
    }
}

impl BusStream for Requests {
    type Item = Result<LauncherRequest, LauncherError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let logins = Pin::new(&mut self.logins).poll_next(cx);
        if let Poll::Ready(Some(signal)) = logins {
            return Poll::Ready(Some(Self::login(signal)));
        }
        let logouts = Pin::new(&mut self.logouts).poll_next(cx);
        match (logins, logouts) {
            (_, Poll::Ready(Some(signal))) => Poll::Ready(Some(Self::logout(signal))),
            (Poll::Ready(None), Poll::Ready(None)) => Poll::Ready(None),
            _ => Poll::Pending,
        }
    }
}
