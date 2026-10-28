//! `org.quire.Accounts1.Launcher` at `/org/quire/Accounts1`: what the agent launcher asks accountd
//! (the registration of its programs, its sessions, the consent sheet for an agent's key, the
//! reports of a login, and the keys it hands to the processes it spawns).
//!
//! Only a connection the caller table knows as the agent launcher may call; every other sender is
//! `AccessDenied`. The signals `AgentLoginRequested`, `AgentLogoutRequested` and
//! `ProcessCredentialRevoked` are sent to the registering connection alone.

use super::launchers::Ask;
use crate::callers::Callers;
use crate::errors::RefusedError;
use crate::host::Host;
use crate::state::Core;
use porter_capability::AgentProgram;
use porter_capability::{LoginOutcome, LoginRequestId};
use porter_core::AgentState;
use porter_core::SessionToken;
use porter_dbus::accounts::LauncherServer;
use porter_dbus::{Call, Details};
use porter_service_accounts::{AgentFault, LoginEnd};
use porter_wire::{ParentWindow, Refusal};
use std::sync::Arc;
use zbus::message::Header;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

/// The launcher object.
pub(crate) struct LauncherObject<H, C>(Arc<Core<H, C>>);

impl<H, C> LauncherObject<H, C> {
    pub(crate) fn new(core: Arc<Core<H, C>>) -> Self {
        Self(core)
    }
}

/// The `parent_window` argument: empty is no parent.
fn window(text: &str) -> ParentWindow {
    match text.is_empty() {
        true => ParentWindow::Unparented,
        false => ParentWindow::Handle(text.to_owned()),
    }
}

fn agent_fault(fault: AgentFault) -> RefusedError {
    match fault {
        AgentFault::UnknownAccount => RefusedError::invalid("no such account"),
        AgentFault::NotAnAgent => {
            RefusedError::invalid("not an account of an agent that signs itself in")
        }
        AgentFault::Unavailable => RefusedError::of(Refusal::Unavailable),
        // a variant a newer porter adds: refused as unavailable
        _ => RefusedError::of(Refusal::Unavailable),
    }
}

impl<H: Host, C: Callers> LauncherServer for LauncherObject<H, C> {
    type Error = RefusedError;

    /// Makes the caller the launcher of these agent programs, for as long as its connection
    /// lives. One launcher per program, first wins (`AlreadyRegistered`); a program the caller
    /// holds already is a no-op, and a later call adds more.
    async fn register_launcher(
        &self,
        Call { header, .. }: Call<'_>,
        programs: Vec<String>,
    ) -> Result<(), RefusedError> {
        let sender = self.0.launcher_of(&header).await?;
        let programs = programs
            .iter()
            .map(|program| AgentProgram::parse(program).map_err(RefusedError::invalid))
            .collect::<Result<Vec<_>, _>>()?;
        if programs.is_empty() {
            return Err(RefusedError::invalid("no program to launch"));
        }
        self.0
            .launchers
            .register(&sender, &programs)
            .await
            .map_err(|fault| RefusedError::launcher(fault, "a launcher holds that program"))
    }

    /// Opens a launcher session for the caller (the agent launcher that registered a program):
    /// the span a permit made "for this session only" lasts. It belongs to this connection; it
    /// ends at `EndSession` or when the connection leaves the bus. A session another connection
    /// holds is `SessionTaken`; one this connection holds is a no-op.
    async fn begin_session(
        &self,
        Call { header, .. }: Call<'_>,
        session: String,
    ) -> Result<(), RefusedError> {
        let owner = self.0.launcher_of(&header).await?;
        let session = SessionToken::parse(&session).map_err(RefusedError::invalid)?;
        self.0
            .launchers
            .begin_session(&owner, session)
            .map_err(|fault| RefusedError::launcher(fault, "cannot open that session"))
    }

    /// Ends a session the caller began: its session permits are removed, the process credentials
    /// under them end `session_closed` (and the caller is told, in the unicast signal) and the
    /// audit records each. A session that is not open, or is another connection's, is
    /// `UnknownSession` and nothing is ended.
    async fn end_session(
        &self,
        Call { header, .. }: Call<'_>,
        session: String,
    ) -> Result<(), RefusedError> {
        let owner = self.0.launcher_of(&header).await?;
        let session = SessionToken::parse(&session).map_err(RefusedError::invalid)?;
        self.0
            .launchers
            .end_session(&owner, &session)
            .map_err(|fault| {
                RefusedError::launcher(fault, "no such session for this connection")
            })?;
        self.0.close_session(&session, true).await;
        Ok(())
    }

    /// The consent sheet for an agent program's key: a Request object answered as the choose
    /// sheet's is. With `session`, an open session of this connection, the sheet also offers
    /// "This session only".
    async fn request_agent_key(
        &self,
        Call { header, .. }: Call<'_>,
        program: String,
        class: String,
        session: String,
        parent_window: String,
        options: Details,
    ) -> Result<OwnedObjectPath, RefusedError> {
        self.0
            .agent_key_sheet(
                &header,
                &program,
                &class,
                &session,
                window(&parent_window),
                &options,
            )
            .await
    }

    /// What the launcher reports of an `AgentLoginRequested`. `ready` sets the account's state
    /// ready before whoever waits is told. Only the connection that was sent the request may
    /// answer it.
    async fn report_agent_login(
        &self,
        Call { header, .. }: Call<'_>,
        request: String,
        outcome: String,
        reason: String,
    ) -> Result<(), RefusedError> {
        self.report(&header, Ask::Login, &request, &outcome, &reason)
            .await
    }

    /// What the launcher reports of an `AgentLogoutRequested`. `ready` (done) sets the account's
    /// state to `needs_login`.
    async fn report_agent_logout(
        &self,
        Call { header, .. }: Call<'_>,
        request: String,
        outcome: String,
        reason: String,
    ) -> Result<(), RefusedError> {
        self.report(&header, Ask::Logout, &request, &outcome, &reason)
            .await
    }

    /// An API key for one process the launcher spawns, under a permit: the credential's id and a
    /// sealed memfd.
    async fn issue_process_credential(
        &self,
        Call { header, .. }: Call<'_>,
        permit: String,
        program: String,
    ) -> Result<(String, OwnedFd), RefusedError> {
        self.0
            .issue_process_credential(&header, &permit, &program)
            .await
    }

    /// The launcher says the process is over; accountd forgets the credential.
    async fn revoke_process_credential(
        &self,
        Call { header, .. }: Call<'_>,
        id: String,
    ) -> Result<(), RefusedError> {
        self.0.revoke_process_credential(&header, &id).await
    }
}

impl<H: Host, C: Callers> LauncherObject<H, C> {
    async fn report(
        &self,
        header: &Header<'_>,
        kind: Ask,
        request: &str,
        outcome: &str,
        reason: &str,
    ) -> Result<(), RefusedError> {
        let sender = self.0.launcher_of(header).await?;
        let request = LoginRequestId::parse(request).map_err(RefusedError::invalid)?;
        // The words are not echoed back: whatever else a launcher sent stays its own.
        let outcome = LoginOutcome::from_wire(outcome, reason)
            .map_err(|_| RefusedError::invalid("not an outcome of a login"))?;
        let pending = self.0.launchers.take(&sender, &request, kind)?;
        // The state first, so whoever waits finds the account as the report left it.
        let state = match (kind, outcome) {
            (Ask::Login, LoginOutcome::Ready) => Some(AgentState::Ready),
            (Ask::Logout, LoginOutcome::Ready) => Some(AgentState::NeedsLogin),
            _ => None,
        };
        let saved = match state {
            Some(state) => self.0.host.set_agent_state(&pending.account, state).await,
            None => Ok(false),
        };
        if saved == Ok(true) {
            self.0.settled().await;
        }
        pending.end(LoginEnd::Reported(outcome));
        saved.map(|_| ()).map_err(agent_fault)
    }
}
