//! What the launcher object shares: the host, the caller table, the roster and the credentials in
//! force, and the checks and endings every method uses (accountd's `Core`, `handoff` and
//! `request` before the launcher moved here).
//!
//! `IssueProcessCredential` and `RevokeProcessCredential` (agent-session ask P2, lane
//! p2-handoff): an API key for one process the agent launcher spawns, for an agent that cannot be
//! pointed at inferd (`Need::Agent { base_url: Present }` is the metered route, P4).
//!
//! - Who: a connection the caller table knows as the agent launcher that holds a live
//!   `RegisterLauncher` registration for the program (`NotRegistered` otherwise). Nobody else:
//!   not an app, not an agent (`AccessDenied`).
//! - What: a permit that is an `Allow` for the Llm kind of the app `org.quire.Agent.<program>`
//!   (P4's audience convention; `UnknownGrant` / `AudienceNotGranted`), on an account that holds
//!   an API key (`ApiKey`, `OAuthMintsKey`; `NotAKeyAccount`), not turned off for Llm and not
//!   waiting to be signed in again. A permit is a grant of the hosted service.
//! - Never `Once`: a `Once` permit is spent by its first use, so a second turn of the agent, which
//!   holds the key for its whole life, would find the permit gone and the key still in the child:
//!   a consent that says "once" must not hand over a key that works until the process ends
//!   (`OnceGrant`). `Always` is accepted, and so is `Session`: a permit "for this session only"
//!   made for a session the same connection holds open (`UnknownSession` otherwise), since the
//!   credential then ends with the session.
//! - It ends at `RevokeProcessCredential`, when the launcher's connection leaves the bus, when
//!   the permit is withdrawn, when the account is removed (checked after every change to the
//!   registry: `Launchers::registry_changed`), or when its permit's launcher session closes
//!   (`session_closed`, told to the launcher only when it is still connected). inferd unlinks a
//!   tmpfs file; it cannot reach a memfd the child has, so it tells the launcher in the unicast
//!   signal `ProcessCredentialRevoked(id, reason)` and the launcher must end the process. Each end
//!   is audited.

use super::credentials::{Credentials, Handle, Issue};
use super::errors::RefusedError;
use super::host::LauncherHost;
use super::roster::Launchers;
use crate::peers::{Peers, Role};
use porter_core::capability::AgentProgram;
use porter_core::consent::{Decision, GrantScope};
use porter_core::wire::Refusal;
use porter_core::{
    Account, AccountState, AppId, AuthKind, CapabilityKind, LauncherSession, ProcessCredentialId,
    Toggle,
};
use porter_core::{AccountId, GrantId, KeyPermitId};
use porter_dbus::LauncherFault;
use porter_service::Registry;
use porter_service::audit::{AuditEvent, CredentialEnd, Handoff};
use porter_sheets::{OpenSheets, RequestObjects};
use std::sync::Arc;
use zbus::message::Header;
use zbus::zvariant::{Fd, OwnedValue, Value};

/// The reverse-DNS prefix of the app a program's permits are held under (inferd's `agent_app`
/// names the same app).
pub(crate) const AGENT_APP_PREFIX: &str = "org.quire.Agent.";

/// What a change to the registry is told to: the key host's own news (Settings rows).
pub(crate) type Changed = Arc<dyn Fn() + Send + Sync>;

/// The shared state of the launcher object.
pub(crate) struct Core<H, P> {
    pub(crate) host: Arc<H>,
    pub(crate) peers: Arc<P>,
    pub(crate) launchers: Launchers,
    pub(crate) credentials: Credentials,
    /// The Request objects of the key sheet (`ServiceRoot::Inference`).
    pub(crate) requests: RequestObjects,
    /// The sheets open now, one per app and kind.
    pub(crate) open_sheets: OpenSheets,
    /// Told after the launcher changed the registry (an agent's state, a session's permits).
    pub(crate) changed: Option<Changed>,
}

/// Whether the account signs in with a key it holds, one `read_key` can read.
fn holds_a_key(auth: AuthKind) -> bool {
    matches!(auth, AuthKind::ApiKey | AuthKind::OAuthMintsKey)
}

/// Whether the key of `account` may leave now: not when the person turned the account off for Llm
/// (`Denied`) nor while it waits to be signed in again (`NeedsReauth`).
fn usable(registry: &Registry, account: &Account) -> Result<(), Refusal> {
    let off = registry.toggles.iter().any(|t| {
        t.account == account.id && t.kind == CapabilityKind::Llm && t.toggle == Toggle::Off
    });
    match (off, account.state) {
        (true, _) => Err(Refusal::Denied),
        (false, AccountState::NeedsReauth) => Err(Refusal::NeedsReauth),
        (false, _) => Ok(()),
    }
}

/// The reply's second member: the descriptor of a memfd (`h`) or the path of a file (`s`).
fn handle_value(handle: Handle) -> Result<OwnedValue, RefusedError> {
    let value = match handle {
        Handle::Fd(fd) => OwnedValue::try_from(Fd::from(fd)),
        Handle::Path(path) => {
            OwnedValue::try_from(Value::from(path.to_string_lossy().into_owned()))
        }
    };
    value.map_err(|why| RefusedError::failed(format!("cannot carry the handle: {why}")))
}

impl<H: LauncherHost, P: Peers> Core<H, P> {
    /// The unique name of the sender, which must be the agent launcher; anyone else is
    /// `AccessDenied`.
    pub(crate) async fn launcher_of(&self, header: &Header<'_>) -> Result<String, RefusedError> {
        let denied = || RefusedError::access_denied("only the agent launcher may make this call");
        let sender = header
            .sender()
            .ok_or_else(|| RefusedError::access_denied("no sender"))?
            .to_string();
        match self.peers.caller_of(&sender).await {
            Some(caller) if caller.role == Role::AgentLauncher => Ok(sender),
            _ => Err(denied()),
        }
    }

    /// Tells the key host what changed: the credentials whose permit or account is gone end, and
    /// the host's own news is told.
    pub(crate) async fn settled(self: &Arc<Self>) {
        self.sweep().await;
        if let Some(changed) = &self.changed {
            changed();
        }
    }

    /// Ends the credentials the registry no longer backs.
    pub(crate) async fn sweep(&self) {
        let registry = self.host.registry();
        self.end_credentials(&registry).await;
    }

    /// `IssueProcessCredential`: the credential's id and its handle.
    pub(crate) async fn issue_process_credential(
        &self,
        header: &Header<'_>,
        permit: &str,
        program: &str,
        target: &str,
    ) -> Result<(String, OwnedValue), RefusedError> {
        let owner = self.launcher_of(header).await?;
        let grant = GrantId::from(KeyPermitId::parse(permit).map_err(RefusedError::invalid)?);
        let program = AgentProgram::parse(program).map_err(RefusedError::invalid)?;
        let handoff = Handoff::from_word(target).ok_or_else(|| {
            RefusedError::invalid(format!("`{target}` is not a way to hand a key"))
        })?;
        if !self.launchers.holds(&owner, &program) {
            return Err(RefusedError::launcher(
                LauncherFault::NotRegistered,
                "this connection has not registered that program",
            ));
        }
        let registry = self.host.registry();
        let held = registry
            .grants
            .iter()
            .find(|g| g.id == grant && g.decision == Decision::Allow)
            .ok_or_else(|| RefusedError::of(Refusal::UnknownGrant))?;
        let audience = held.key.app.clone();
        if audience.name.as_str() != format!("{AGENT_APP_PREFIX}{program}")
            || held.key.kind != CapabilityKind::Llm
        {
            return Err(RefusedError::of(Refusal::AudienceNotGranted));
        }
        match &held.scope {
            GrantScope::Always => {}
            GrantScope::Once => {
                return Err(RefusedError::launcher(
                    LauncherFault::OnceGrant,
                    "a once permit is spent by its first use; a process holds its key for its life",
                ));
            }
            GrantScope::Session(session) => {
                if !self.launchers.session_open(&owner, session) {
                    return Err(RefusedError::launcher(
                        LauncherFault::UnknownSession,
                        "the permit is for a session this connection does not hold open",
                    ));
                }
            }
        }
        let account = registry
            .accounts
            .iter()
            .find(|a| a.id == held.key.account)
            .ok_or_else(|| RefusedError::of(Refusal::UnknownGrant))?;
        if !holds_a_key(account.auth) {
            return Err(RefusedError::launcher(
                LauncherFault::NotAKeyAccount,
                "the account holds no API key to hand over",
            ));
        }
        usable(&registry, account).map_err(RefusedError::of)?;
        let key = self
            .host
            .read_key(&account.id)
            .await
            .map_err(RefusedError::of)?;
        let (id, handle) = self
            .credentials
            .issue(&Issue {
                owner: &owner,
                audience: &audience,
                account: &account.id,
                grant: &grant,
                key: &key,
                handoff,
            })
            .map_err(RefusedError::of)?;
        // The permit may have gone while the key was read, and its session may have closed: a
        // credential must not outlive either (`close_session` ends the ones already made).
        let still = self.host.registry().grants.iter().any(|g| {
            g.id == grant
                && g.decision == Decision::Allow
                && g.scope
                    .session()
                    .is_none_or(|session| self.launchers.session_open(&owner, session))
        });
        if !still {
            // Never handed over, so not audited as ended.
            self.credentials.take(&owner, &id, |_| {});
            return Err(RefusedError::of(Refusal::UnknownGrant));
        }
        self.launchers.note(
            &audience,
            &account.id,
            AuditEvent::ProcessCredentialIssued {
                audience: porter_core::Audience(audience.name.as_str().to_owned()),
                handoff,
            },
        );
        Ok((id.to_string(), handle_value(handle)?))
    }

    /// `RevokeProcessCredential`: the launcher says the process is over (or killed it). Audited
    /// as `process_exited`; no signal, the launcher knows.
    pub(crate) async fn revoke_process_credential(
        &self,
        header: &Header<'_>,
        id: &str,
    ) -> Result<(), RefusedError> {
        let owner = self.launcher_of(header).await?;
        let id = ProcessCredentialId::parse(id).map_err(RefusedError::invalid)?;
        self.credentials
            .take(&owner, &id, |held| {
                self.note_end(&held.audience, &held.account, CredentialEnd::ProcessExited);
            })
            .ok_or_else(|| {
                RefusedError::launcher(
                    LauncherFault::UnknownCredential,
                    "no such credential for this connection",
                )
            })?;
        Ok(())
    }

    /// Ends the credentials whose permit or account the registry no longer has, tells each
    /// launcher, and audits. Called after every change to the registry.
    pub(crate) async fn end_credentials(&self, registry: &Registry) {
        let gone = |grant: &GrantId| {
            !registry
                .grants
                .iter()
                .any(|g| g.id == *grant && g.decision == Decision::Allow)
        };
        let account_gone =
            |account: &AccountId| !registry.accounts.iter().any(|a| a.id == *account);
        let ended = self.credentials.end_where(
            |held| match gone(&held.grant) {
                true if account_gone(&held.account) => Some(CredentialEnd::AccountRemoved),
                true => Some(CredentialEnd::GrantRevoked),
                false => None,
            },
            |held, reason| self.note_end(&held.audience, &held.account, reason),
        );
        for (id, held, reason) in ended {
            self.launchers
                .tell_revoked(&held.owner, id.as_str(), reason.word())
                .await;
        }
    }

    /// A launcher session is over (`EndSession`, or the connection that held it left; the table
    /// no longer lists it): the process credentials issued under its permits end
    /// `session_closed` and the permits are removed, audited. `tell` says the launcher is still
    /// connected to hear `ProcessCredentialRevoked`. The credentials end before the permits go,
    /// so none is ever reported as `grant_revoked`.
    pub(crate) async fn close_session(self: &Arc<Self>, session: &LauncherSession, tell: bool) {
        let grants: Vec<GrantId> = self
            .host
            .registry()
            .grants
            .iter()
            .filter(|g| g.scope.session() == Some(session))
            .map(|g| g.id.clone())
            .collect();
        let ended = self.credentials.end_where(
            |held| {
                grants
                    .contains(&held.grant)
                    .then_some(CredentialEnd::SessionClosed)
            },
            |held, reason| self.note_end(&held.audience, &held.account, reason),
        );
        for (id, held, reason) in ended {
            if tell {
                self.launchers
                    .tell_revoked(&held.owner, id.as_str(), reason.word())
                    .await;
            }
        }
        self.host.end_session_grants(Some(session)).await;
        self.settled().await;
    }

    /// A launcher's connection left the bus: its credentials end; nobody is left to tell.
    pub(crate) fn credentials_left(&self, name: &str) {
        self.credentials.end_where(
            |held| (held.owner == name).then_some(CredentialEnd::LauncherGone),
            |held, reason| self.note_end(&held.audience, &held.account, reason),
        );
    }

    /// Forgets a connection that left the bus. Its launcher sessions close first (their permits
    /// are removed and the credentials under them end `session_closed`, with nobody left to
    /// tell), then what is left of its credentials ends `launcher_gone`.
    pub(crate) async fn left(self: &Arc<Self>, name: &str) {
        for session in self.launchers.left(name) {
            self.close_session(&session, false).await;
        }
        self.credentials_left(name);
    }

    fn note_end(&self, audience: &AppId, account: &AccountId, reason: CredentialEnd) {
        self.launchers.note(
            audience,
            account,
            AuditEvent::ProcessCredentialRevoked {
                audience: porter_core::Audience(audience.name.as_str().to_owned()),
                reason,
            },
        );
    }
}
