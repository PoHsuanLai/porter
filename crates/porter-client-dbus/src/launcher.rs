//! The launcher's side of an agent's login (accountd's `Launcher` over the bus; it is this crate's and
//! not `Porter`'s, because the key it hands over is a sealed memfd, a Linux descriptor) (design/31 R7-R9, C5, C6). The agent launcher
//! (docket-acp) registers the programs it runs, hears when accountd asks it to sign an agent in or
//! out, has the agent do it, and reports only a coarse [`LoginOutcome`]. Nothing of the login
//! itself (a token, a URL, a code) crosses to porter: there is no way to send one.
//!
//! The launcher talks to accountd (`org.quire.Accounts1`), which holds the AI keys: whoever uses a
//! credential holds it. An agent's yes to use one of the person's keys is a permit
//! ([`GrantId`]), asked for with [`Launcher::request_agent_key`].
//!
//! For an agent that cannot be pointed at inferd the launcher can also be handed an API key for
//! the process it spawns ([`Launcher::issue_credential`]; the duties and the residual risk are in
//! [`crate::credential`]).
//!
//! The caller must be known to accountd as the agent launcher (`CallerRole::AgentLauncher`, a row
//! of accountd's caller table a machine writes); any other caller is `Link(LinkError::Denied)`.
//!
//! ```ignore
//! let launcher = Bus::over(connection).launcher();
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

use crate::Bus;
use crate::credential::{ProcessCredential, Revoked};
use crate::failure::{launcher_error, link_error};
use futures_util::StreamExt;
use futures_util::stream::select;
use porter_capability::AgentProgram;
use porter_capability::{Capability, Subject};
use porter_capability::{LoginOutcome, LoginRequestId};
use porter_client::{Changes, LinkError, Malformed};
use porter_core::audit::CredentialEnd;
use porter_core::{AccountId, GrantId, ProviderId};
use porter_core::{AccountLabel, Candidate, DataClass, Restriction};
use porter_core::{ProcessCredentialId, SessionToken};
use porter_dbus::accounts::sheet::{SheetKind, reply_of};
use porter_dbus::accounts::{
    Abandon, AgentLoginRequested, AgentLogoutRequested, LauncherProxy, ProcessCredentialRevoked,
    ServiceRoot, Sheet, SheetError,
};
use porter_ids::word::{from_word, word as word_of};
use porter_wire::AccountsReply;
use porter_wire::LauncherFault;
use porter_wire::Refusal;

/// Why a launcher call failed. More reasons may be added: match with a wildcard.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LauncherError {
    /// accountd refused the call for a reason of the launcher interface (a request, a program, a
    /// permit, a credential or a session that is not the connection's to use).
    #[error("{0}")]
    Fault(LauncherFault),
    /// accountd refused the permit or the account for a reason an app also meets: `UnknownGrant`,
    /// `AudienceNotGranted` (the permit is not for `org.quire.Agent.<program>`), `NeedsReauth`,
    /// `Denied` (the person turned the account off), `Unavailable`.
    #[error("refused")]
    Refused(Refusal),
    /// The call did not complete: accountd is not there, did not know the caller as the agent
    /// launcher, refused an argument, or sent something that is not porter's protocol.
    #[error(transparent)]
    Link(#[from] LinkError),
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

fn malformed(what: Malformed) -> LauncherError {
    LauncherError::Link(LinkError::Malformed(what))
}

impl Bus {
    async fn launcher_proxy(&self) -> Result<LauncherProxy<'static>, LauncherError> {
        self.accounts
            .proxy(&self.connection)
            .await
            .map_err(|e| LauncherError::Link(link_error(&e)))
    }
}

fn sheet_error(error: SheetError) -> LauncherError {
    match error {
        SheetError::Bus(error) => launcher_error(&error),
        SheetError::Gone => LauncherError::Link(LinkError::Unreachable),
        SheetError::Unreadable => malformed(Malformed::Sheet),
        // a failure a later porter-dbus adds
        _ => malformed(Malformed::UnknownFailure),
    }
}

fn request(
    kind: AskKind,
    request: &str,
    account: &str,
    program: &str,
) -> Result<LauncherRequest, Malformed> {
    Ok(LauncherRequest {
        kind,
        request: LoginRequestId::parse(request).map_err(|_| Malformed::Id)?,
        account: AccountId::parse(account).map_err(|_| Malformed::Id)?,
        program: AgentProgram::parse(program).map_err(|_| Malformed::Id)?,
    })
}

fn login(signal: AgentLoginRequested) -> Result<LauncherRequest, Malformed> {
    let args = signal.args().map_err(|_| Malformed::Signal)?;
    request(AskKind::Login, &args.request, &args.account, &args.program)
}

fn logout(signal: AgentLogoutRequested) -> Result<LauncherRequest, Malformed> {
    let args = signal.args().map_err(|_| Malformed::Signal)?;
    request(AskKind::Logout, &args.request, &args.account, &args.program)
}

fn revoked(signal: ProcessCredentialRevoked) -> Result<Revoked, Malformed> {
    let args = signal.args().map_err(|_| Malformed::Signal)?;
    Ok(Revoked {
        id: ProcessCredentialId::parse(&args.id).map_err(|_| Malformed::Id)?,
        reason: from_word::<CredentialEnd>(&args.reason).ok_or(Malformed::Signal)?,
    })
}

/// The launcher's calls to accountd, over `bus`.
#[derive(Debug, Clone)]
pub struct Launcher {
    bus: Bus,
}

impl Bus {
    /// The agent launcher's calls to accountd over this connection.
    pub fn launcher(&self) -> Launcher {
        Launcher { bus: self.clone() }
    }
}

impl Launcher {
    /// Opens `session`, the span a permit "for this session only" lasts (the launcher names it,
    /// usually from an ACP session id). It belongs to this connection, which must have
    /// registered a program, and ends at [`Launcher::end_session`] or when the connection
    /// leaves the bus. A session another connection holds is [`LauncherFault::SessionTaken`];
    /// beginning one this connection holds is a no-op.
    pub async fn begin_session(&self, session: &SessionToken) -> Result<(), LauncherError> {
        self.bus
            .launcher_proxy()
            .await?
            .begin_session(session.as_str())
            .await
            .map_err(|e| launcher_error(&e))
    }

    /// Ends a session this connection began: accountd removes the permits scoped to it and ends
    /// the process credentials under them (`CredentialEnd::SessionClosed`), telling this
    /// connection of each in [`Launcher::revocations`] so the launcher can end the processes. A
    /// session that is not open for this connection is [`LauncherFault::UnknownSession`].
    pub async fn end_session(&self, session: &SessionToken) -> Result<(), LauncherError> {
        self.bus
            .launcher_proxy()
            .await?
            .end_session(session.as_str())
            .await
            .map_err(|e| launcher_error(&e))
    }

    /// Asks the person, on accountd's consent sheet titled for `program`, which of their AI keys
    /// the agent may use (interactive use). With `session`, an open session of this connection,
    /// the sheet also offers "This session only", and a permit made so is scoped to it. Returns
    /// the chosen account and its permit; a person who closes the sheet or refuses is
    /// [`LauncherError::Refused`] (`Dismissed`, `Denied`). The program must be one this
    /// connection registered.
    pub async fn request_agent_key(
        &self,
        program: &AgentProgram,
        class: DataClass,
        session: Option<&SessionToken>,
    ) -> Result<KeyCandidate, LauncherError> {
        let proxy = self.bus.launcher_proxy().await?;
        let mut listener = Sheet::subscribe_on(&self.bus.connection, ServiceRoot::Accounts)
            .await
            .map_err(|e| launcher_error(&e))?;
        // Armed before the call: a dropped call may already have made the sheet.
        let mut abandon = Abandon(listener.closer(None));
        let path = proxy
            .request_agent_key(
                program.as_str(),
                word_of(&class),
                session.map_or("", SessionToken::as_str),
                "",
                &listener.options(),
            )
            .await;
        let path = match path {
            Ok(path) => path,
            Err(error) => {
                abandon.answered();
                return Err(launcher_error(&error));
            }
        };
        abandon.0 = listener.closer(Some(path.clone()));
        let (code, results) = listener.response(&path).await.map_err(sheet_error)?;
        abandon.answered();
        match reply_of(SheetKind::Choose, code, results).map_err(|_| malformed(Malformed::Reply))? {
            AccountsReply::Chosen(candidate) => Ok(candidate.into()),
            AccountsReply::Refused(refusal) => Err(LauncherError::Refused(refusal)),
            _ => Err(malformed(Malformed::Reply)),
        }
    }

    /// An API key for a process this launcher is about to spawn for `program`, under `permit`
    /// (the person's yes for the app `org.quire.Agent.<program>` to use a key, scope always, or
    /// "this session only" for a session this connection holds open), handed over on
    /// a sealed memfd to pass to the child, the one way a key is handed over. The program
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
    ) -> Result<ProcessCredential, LauncherError> {
        let (id, handle) = self
            .bus
            .launcher_proxy()
            .await?
            .issue_process_credential(permit.as_str(), program.as_str())
            .await
            .map_err(|e| launcher_error(&e))?;
        let id = ProcessCredentialId::parse(&id).map_err(|_| malformed(Malformed::Id))?;
        Ok(ProcessCredential::new(id, handle.into()))
    }

    /// Ends a credential this connection was issued, because the process that held it exited or
    /// was killed: accountd forgets it. Not for a credential accountd
    /// ended itself (that is [`LauncherFault::UnknownCredential`]).
    pub async fn revoke_credential(&self, id: &ProcessCredentialId) -> Result<(), LauncherError> {
        self.bus
            .launcher_proxy()
            .await?
            .revoke_process_credential(id.as_str())
            .await
            .map_err(|e| launcher_error(&e))
    }

    /// What accountd tells this connection when it ends a credential itself (the permit was
    /// revoked, the account removed): the launcher must end that process. Subscribe before
    /// issuing, so none is missed.
    pub async fn revocations(&self) -> Result<Changes<Revoked>, LauncherError> {
        let stream = self
            .bus
            .launcher_proxy()
            .await?
            .receive_process_credential_revoked()
            .await
            .map_err(|e| launcher_error(&e))?;
        Ok(Changes::new(stream.map(revoked)))
    }

    /// Registers the programs this connection launches, for as long as it lives. A program
    /// another live connection holds is [`LauncherFault::AlreadyRegistered`] and none of the list
    /// is taken; registering a program this connection holds is a no-op, and a later call adds
    /// more.
    pub async fn register(&self, programs: &[AgentProgram]) -> Result<(), LauncherError> {
        let names: Vec<&str> = programs.iter().map(AgentProgram::as_str).collect();
        self.bus
            .launcher_proxy()
            .await?
            .register_launcher(&names)
            .await
            .map_err(|e| launcher_error(&e))
    }

    /// The requests accountd sends this connection. Subscribe before registering, so none is
    /// missed.
    pub async fn requests(&self) -> Result<Changes<LauncherRequest>, LauncherError> {
        let proxy = self.bus.launcher_proxy().await?;
        let logins = proxy
            .receive_agent_login_requested()
            .await
            .map_err(|e| launcher_error(&e))?;
        let logouts = proxy
            .receive_agent_logout_requested()
            .await
            .map_err(|e| launcher_error(&e))?;
        Ok(Changes::new(select(logins.map(login), logouts.map(logout))))
    }

    /// Reports what became of a login request. Only the connection that was sent it may.
    pub async fn report_login(
        &self,
        request: &LoginRequestId,
        outcome: LoginOutcome,
    ) -> Result<(), LauncherError> {
        let (word, reason) = outcome.to_wire();
        self.bus
            .launcher_proxy()
            .await?
            .report_agent_login(request.as_str(), word, reason)
            .await
            .map_err(|e| launcher_error(&e))
    }

    /// Reports what became of a logout request; `Ready` means the agent is signed out.
    pub async fn report_logout(
        &self,
        request: &LoginRequestId,
        outcome: LoginOutcome,
    ) -> Result<(), LauncherError> {
        let (word, reason) = outcome.to_wire();
        self.bus
            .launcher_proxy()
            .await?
            .report_agent_logout(request.as_str(), word, reason)
            .await
            .map_err(|e| launcher_error(&e))
    }
}
