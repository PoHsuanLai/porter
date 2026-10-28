//! The launcher's side of an agent's login (design/31 R7-R9, C5, C6). The agent launcher
//! (docket-acp) registers the programs it runs, hears when inferd asks it to sign an agent in or
//! out, has the agent do it, and reports only a coarse [`LoginOutcome`]. Nothing of the login
//! itself (a token, a URL, a code) crosses to porter: there is no way to send one.
//!
//! The launcher talks to inferd (`org.quire.Inference1`), which holds the AI keys: whoever uses a
//! credential holds it. An agent's yes to use one of the person's keys is a permit
//! ([`GrantId`]), asked for with [`Launcher::request_agent_key`].
//!
//! For an agent that cannot be pointed at inferd the launcher can also be handed an API key for
//! the process it spawns ([`Launcher::issue_credential`]; the duties and the residual risk are in
//! [`crate::credential`]).
//!
//! The caller must be known to inferd as the agent launcher (`CallerRole::AgentLauncher`, a row
//! of inferd's caller table a machine writes); any other caller is [`CallFailure::Denied`].
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

use crate::abandon::Abandon;
use crate::call_failure::{CallFailure, bus_failure_from};
use crate::credential::{CredentialHandle, ProcessCredential, Revocations};
use porter_capability::AgentProgram;
use porter_capability::{Capability, Subject};
use porter_capability::{LoginOutcome, LoginRequestId};
use porter_core::audit::Handoff;
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{AccountLabel, Candidate, CoreError, DataClass, Restriction};
use porter_core::{LauncherSession, ProcessCredentialId};
use porter_dbus::sheet::{SheetKind, reply_of};
use porter_dbus::{
    AgentLoginRequested, AgentLoginRequestedStream, AgentLogoutRequested,
    AgentLogoutRequestedStream, BusConnection, BusError, BusStream, LauncherFault, LauncherProxy,
    ServiceRoot, Sheet, SheetError, launcher_fault_of, refusal_of, zvariant::Value,
};
use porter_ids::IdError;
use porter_ids::word::word as word_of;
use porter_wire::AccountsReply;
use porter_wire::Refusal;
use std::os::fd::AsFd;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Why a launcher call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LauncherError {
    /// inferd refused the call for a reason of the launcher interface (a request, a program, a
    /// permit, a credential or a session that is not the connection's to use).
    #[error("{0}")]
    Fault(LauncherFault),
    /// inferd refused the permit or the account for a reason an app also meets: `UnknownGrant`,
    /// `AudienceNotGranted` (the permit is not for `org.quire.Agent.<program>`), `NeedsReauth`,
    /// `Denied` (the person turned the account off), `Unavailable`.
    #[error("refused: {0:?}")]
    Refused(Refusal),
    /// The call itself failed: inferd is not there, did not know the caller as the agent
    /// launcher, refused an argument, or sent something that is not porter's protocol.
    #[error(transparent)]
    Bus(CallFailure),
}

impl From<BusError> for LauncherError {
    fn from(error: BusError) -> Self {
        match (launcher_fault_of(&error), refusal_of(&error)) {
            (Some(fault), _) => LauncherError::Fault(fault),
            (None, Some(refusal)) => LauncherError::Refused(refusal),
            (None, None) => LauncherError::Bus(error.into()),
        }
    }
}

bus_failure_from!(LauncherError: CallFailure, CoreError, IdError);

/// What inferd asks of the launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    /// Have the agent sign itself in.
    Login,
    /// Have the agent sign itself out.
    Logout,
}

/// One request inferd sent to this launcher alone.
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

/// The key the person chose for an agent, and the permit that lets the agent use it: what
/// [`Launcher::request_agent_key`] answers. It is a `Candidate` with the permit where a grant
/// was.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyCandidate {
    /// The account that holds the key.
    pub account: AccountId,
    /// Its label.
    pub label: AccountLabel,
    /// Its provider, for the mark.
    pub provider: ProviderId,
    /// The account, or the model of it, that fits.
    pub subject: Subject,
    /// The capability that fits.
    pub capability: Capability,
    /// What limits it.
    pub restriction: Restriction,
    /// The person's yes for this agent to use the key; `issue_credential` is asked by it.
    pub permit: GrantId,
}

impl From<Candidate> for KeyCandidate {
    fn from(candidate: Candidate) -> Self {
        Self {
            account: candidate.account,
            label: candidate.label,
            provider: candidate.provider,
            subject: candidate.subject,
            capability: candidate.capability,
            restriction: candidate.restriction,
            permit: candidate.grant,
        }
    }
}

/// The launcher's calls to inferd.
#[derive(Debug, Clone)]
pub struct Launcher {
    connection: BusConnection,
    launcher: LauncherProxy<'static>,
}

/// A data class as its slug, the form the bus takes.
fn slug(class: DataClass) -> Result<String, LauncherError> {
    match serde_json::to_value(class) {
        Ok(serde_json::Value::String(slug)) => Ok(slug),
        _ => Err(CallFailure::Malformed("a data class with no slug".into()).into()),
    }
}

impl Launcher {
    /// A launcher over `connection`, which inferd (`org.quire.Inference1`) knows as the agent
    /// launcher.
    pub async fn connect(connection: &BusConnection) -> Result<Self, LauncherError> {
        Ok(Self {
            connection: connection.clone(),
            launcher: LauncherProxy::new(connection).await?,
        })
    }

    /// Opens `session`, the span a permit "for this session only" lasts (the launcher names it,
    /// usually from an ACP session id). It belongs to this connection, which must have
    /// registered a program, and ends at [`Launcher::end_session`] or when the connection
    /// leaves the bus. A session another connection holds is [`LauncherFault::SessionTaken`];
    /// beginning one this connection holds is a no-op.
    pub async fn begin_session(&self, session: &LauncherSession) -> Result<(), LauncherError> {
        Ok(self.launcher.begin_session(session.as_str()).await?)
    }

    /// Ends a session this connection began: inferd removes the permits scoped to it and ends
    /// the process credentials under them (`CredentialEnd::SessionClosed`), telling this
    /// connection of each in [`Launcher::revocations`] so the launcher can end the processes. A
    /// session that is not open for this connection is [`LauncherFault::UnknownSession`].
    pub async fn end_session(&self, session: &LauncherSession) -> Result<(), LauncherError> {
        Ok(self.launcher.end_session(session.as_str()).await?)
    }

    /// Asks the person, on inferd's consent sheet titled for `program`, which of their AI keys
    /// the agent may use (interactive use). With `session`, an open session of this connection,
    /// the sheet also offers "This session only", and a permit made so is scoped to it. Returns
    /// the chosen account and its permit; a person who closes the sheet or refuses is
    /// [`LauncherError::Refused`] (`Dismissed`, `Denied`). The program must be one this
    /// connection registered.
    pub async fn request_agent_key(
        &self,
        program: &AgentProgram,
        class: DataClass,
        session: Option<&LauncherSession>,
    ) -> Result<KeyCandidate, LauncherError> {
        let mut listener = Sheet::subscribe_on(&self.connection, ServiceRoot::Inference).await?;
        // Armed before the call: a dropped call may already have made the sheet.
        let mut abandon = Abandon(listener.closer(None));
        let path = self
            .launcher
            .request_agent_key(
                program.as_str(),
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
                SheetError::Gone => CallFailure::Unreachable.into(),
                SheetError::Unreadable => {
                    CallFailure::Malformed("a Response that is not (u, a{sv})".into()).into()
                }
                // a failure a later porter-dbus adds
                _ => CallFailure::Malformed("a sheet failure this client does not know".into())
                    .into(),
            })?;
        abandon.0 = None;
        match reply_of(SheetKind::Choose, code, results)? {
            AccountsReply::Chosen(candidate) => Ok(candidate.into()),
            AccountsReply::Refused(refusal) => Err(LauncherError::Refused(refusal)),
            other => {
                Err(CallFailure::Malformed(format!("a chooser answered with {other:?}")).into())
            }
        }
    }

    /// An API key for a process this launcher is about to spawn for `program`, under `permit`
    /// (the person's yes for the app `org.quire.Agent.<program>` to use a key, scope always, or
    /// "this session only" for a session this connection holds open), handed over by `target`: a
    /// sealed memfd to pass to the child, or a 0600 file under `$XDG_RUNTIME_DIR`. The program
    /// must be one this connection registered. A credential under a session permit ends when
    /// the session does ([`Launcher::end_session`], or this connection leaving).
    ///
    /// For an agent that cannot be pointed at inferd (P4, the metered route, needs none of this).
    /// The key is outside porter's meter and the child can read its own key: see the module
    /// documentation of [`crate::credential`] for the launcher's duties.
    pub async fn issue_credential(
        &self,
        permit: &GrantId,
        program: &AgentProgram,
        target: Handoff,
    ) -> Result<ProcessCredential, LauncherError> {
        let (id, handle) = self
            .launcher
            .issue_process_credential(permit.as_str(), program.as_str(), word_of(&target))
            .await?;
        let id = ProcessCredentialId::parse(&id)?;
        let handle = match (target, &*handle) {
            (Handoff::Memfd, Value::Fd(fd)) => CredentialHandle::Memfd(
                fd.as_fd()
                    .try_clone_to_owned()
                    .map_err(|why| CallFailure::Malformed(why.to_string()))?,
            ),
            (Handoff::TmpfsFile, Value::Str(path)) => {
                CredentialHandle::TmpfsFile(path.as_str().into())
            }
            _ => {
                return Err(CallFailure::Malformed(
                    "the handle is not the kind asked for".to_owned(),
                )
                .into());
            }
        };
        Ok(ProcessCredential::new(id, handle))
    }

    /// Ends a credential this connection was issued, because the process that held it exited or
    /// was killed: a tmpfs file is unlinked and inferd forgets it. Not for a credential inferd
    /// ended itself (that is [`LauncherFault::UnknownCredential`]).
    pub async fn revoke_credential(&self, id: &ProcessCredentialId) -> Result<(), LauncherError> {
        Ok(self.launcher.revoke_process_credential(id.as_str()).await?)
    }

    /// What inferd tells this connection when it ends a credential itself (the permit was
    /// revoked, the account removed): the launcher must end that process. Subscribe before
    /// issuing, so none is missed.
    pub async fn revocations(&self) -> Result<Revocations, LauncherError> {
        Ok(Revocations::new(
            self.launcher.receive_process_credential_revoked().await?,
        ))
    }

    /// Registers the programs this connection launches, for as long as it lives. A program
    /// another live connection holds is [`LauncherFault::AlreadyRegistered`] and none of the list
    /// is taken; registering a program this connection holds is a no-op, and a later call adds
    /// more.
    pub async fn register(&self, programs: &[AgentProgram]) -> Result<(), LauncherError> {
        let names: Vec<&str> = programs.iter().map(AgentProgram::as_str).collect();
        Ok(self.launcher.register_launcher(&names).await?)
    }

    /// The requests inferd sends this connection. Subscribe before registering, so none is
    /// missed.
    pub async fn requests(&self) -> Result<Requests, LauncherError> {
        Ok(Requests {
            logins: self.launcher.receive_agent_login_requested().await?,
            logouts: self.launcher.receive_agent_logout_requested().await?,
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
            .launcher
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
            .launcher
            .report_agent_logout(request.as_str(), word, reason)
            .await?)
    }
}

/// The stream of [`LauncherRequest`]s; an item is an error when inferd sent one that is not
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
