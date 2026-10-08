//! `Tokens.IssueProcessCredential` and `Tokens.RevokeProcessCredential` (agent-session ask P2,
//! lane p2-handoff): an API key for one process the agent launcher spawns, for an agent that
//! cannot be pointed at inferd (`Need::Agent { base_url: Present }` is the metered route, P4).
//!
//! - Who: a connection the caller table knows as `AgentLauncher` that holds a live
//!   `RegisterLauncher` registration for the program (`NotRegistered` otherwise). Nobody else:
//!   not a porter daemon, not an app, not an agent (`AccessDenied`).
//! - What: a grant that is an `Allow` for the Llm kind of the app `org.quire.Agent.<program>`
//!   (P4's audience convention; `UnknownGrant` / `AudienceNotGranted`), on an account that holds
//!   an API key (`ApiKey`, `OAuthMintsKey`; `NotAKeyAccount`), not turned off for Llm and not
//!   waiting to be signed in again.
//! - Never `Once`: a `Once` grant is spent by its first use, so a second turn of the agent, which
//!   holds the key for its whole life, would find the grant gone and the key still in the child:
//!   a consent that says "once" must not hand over a key that works until the process ends
//!   (`OnceGrant`). `Always` is accepted, and so is `Session`: a grant "for this session only"
//!   made for a session the same connection holds open (`UnknownSession` otherwise), since the
//!   credential then ends with the session (the match below has no catch-all arm).
//! - It ends at `RevokeProcessCredential`, when the launcher's connection leaves the bus, when
//!   the grant is withdrawn, when the account is removed (checked after every change to the
//!   registry), or when its grant's launcher session closes (`close_session`: `session_closed`,
//!   told to the launcher only when it is still connected). accountd unlinks a tmpfs file; it
//!   cannot reach a memfd the child has, so it tells the launcher in the unicast signal
//!   `ProcessCredentialRevoked(id, reason)` and the launcher must end the process. Each end is
//!   audited.

use crate::callers::Callers;
use crate::core::{Core, Host, Standing};
use crate::credentials::{Handle, Issue};
use crate::errors::RefusedError;
use crate::keys::usable;
use porter_core::audit::{CredentialEnd, Handoff};
use porter_core::capability::AgentProgram;
use porter_core::consent::{Decision, GrantScope};
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AppId, AuthKind, CapabilityKind, GrantId, LauncherSession, ProcessCredentialId,
};
use porter_dbus::{ACCOUNTS_PATH, LauncherFault};
use porter_service::Registry;
use std::sync::Arc;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{Fd, OwnedValue, Value};

/// The interface the revocation signal is on.
const TOKENS_INTERFACE: &str = "org.quire.Accounts1.Tokens";

/// The reverse-DNS prefix of the app a program's grants are held under (inferd's
/// `agent_app` names the same app).
const AGENT_APP_PREFIX: &str = "org.quire.Agent.";

/// Whether the account signs in with a key it holds, one `Peer.ResolveKey` could also read.
fn holds_a_key(auth: AuthKind) -> bool {
    matches!(auth, AuthKind::ApiKey | AuthKind::OAuthMintsKey)
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

impl<H: Host, C: Callers> Core<H, C> {
    /// The unique name of the sender, which must be the agent launcher; anyone else is
    /// `AccessDenied`.
    pub(crate) async fn launcher_of(&self, header: &Header<'_>) -> Result<String, RefusedError> {
        let caller = self.identify(header, Standing::Launching).await?;
        if caller.role != porter_dbus::CallerRole::AgentLauncher {
            return Err(RefusedError::access_denied(
                "only the agent launcher may make this call",
            ));
        }
        header
            .sender()
            .map(|sender| sender.to_string())
            .ok_or_else(|| RefusedError::access_denied("no sender"))
    }

    /// `Tokens.IssueProcessCredential`: the credential's id and its handle.
    pub(crate) async fn issue_process_credential(
        &self,
        header: &Header<'_>,
        grant: &str,
        program: &str,
        target: &str,
    ) -> Result<(String, OwnedValue), RefusedError> {
        let owner = self.launcher_of(header).await?;
        let grant = GrantId::parse(grant).map_err(RefusedError::invalid)?;
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
        let desk = self
            .keys
            .as_ref()
            .ok_or_else(|| RefusedError::of(Refusal::Unavailable))?;
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
                    "a once grant is spent by its first use; a process holds its key for its life",
                ));
            }
            GrantScope::Session(session) => {
                if !self.launchers.session_open(&owner, session) {
                    return Err(RefusedError::launcher(
                        LauncherFault::UnknownSession,
                        "the grant is for a session this connection does not hold open",
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
        let key = desk.read(&account.id).await.map_err(RefusedError::of)?;
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
        // The grant may have gone while the key was read, and its session may have closed: a
        // credential must not outlive either (`close_session` ends the ones already made).
        let still = self.host.registry().grants.iter().any(|g| {
            g.id == grant
                && g.decision == Decision::Allow
                && g.scope
                    .session()
                    .is_none_or(|session| self.launchers.session_open(&owner, session))
        });
        if !still {
            self.credentials.take(&owner, &id);
            return Err(RefusedError::of(Refusal::UnknownGrant));
        }
        desk.note_handoff(&audience, &account.id, handoff);
        Ok((id.to_string(), handle_value(handle)?))
    }

    /// `Tokens.RevokeProcessCredential`: the launcher says the process is over (or killed it).
    /// Audited as `process_exited`; no signal, the launcher knows.
    pub(crate) async fn revoke_process_credential(
        &self,
        header: &Header<'_>,
        id: &str,
    ) -> Result<(), RefusedError> {
        let owner = self.launcher_of(header).await?;
        let id = ProcessCredentialId::parse(id).map_err(RefusedError::invalid)?;
        let held = self.credentials.take(&owner, &id).ok_or_else(|| {
            RefusedError::launcher(
                LauncherFault::UnknownCredential,
                "no such credential for this connection",
            )
        })?;
        self.note_end(&held.audience, &held.account, CredentialEnd::ProcessExited);
        Ok(())
    }

    /// Ends the credentials whose grant or account the registry no longer has, tells each
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
        let ended = self.credentials.end_where(|held| match gone(&held.grant) {
            true if account_gone(&held.account) => Some(CredentialEnd::AccountRemoved),
            true => Some(CredentialEnd::GrantRevoked),
            false => None,
        });
        for (id, held, reason) in ended {
            self.note_end(&held.audience, &held.account, reason);
            self.tell_revoked(&held.owner, &id, reason).await;
        }
    }

    /// A launcher session is over (`EndSession`, or the connection that held it left; the table
    /// no longer lists it): the process credentials issued under its grants end
    /// `session_closed` and the grants are removed, audited. `tell` says the launcher is still
    /// connected to hear `ProcessCredentialRevoked`. The credentials end before the grants go,
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
        let ended = self.credentials.end_where(|held| {
            grants
                .contains(&held.grant)
                .then_some(CredentialEnd::SessionClosed)
        });
        for (id, held, reason) in ended {
            self.note_end(&held.audience, &held.account, reason);
            if tell {
                self.tell_revoked(&held.owner, &id, reason).await;
            }
        }
        self.host.end_session_grants(Some(session)).await;
        self.publish().await;
    }

    /// A launcher's connection left the bus: its credentials end; nobody is left to tell.
    pub(crate) fn credentials_left(&self, name: &str) {
        let ended = self
            .credentials
            .end_where(|held| (held.owner == name).then_some(CredentialEnd::LauncherGone));
        for (_, held, reason) in ended {
            self.note_end(&held.audience, &held.account, reason);
        }
    }

    fn note_end(&self, audience: &AppId, account: &AccountId, reason: CredentialEnd) {
        if let Some(desk) = &self.keys {
            desk.note_handoff_end(audience, account, reason);
        }
    }

    /// `ProcessCredentialRevoked(id, reason)`, to `owner` alone.
    async fn tell_revoked(&self, owner: &str, id: &ProcessCredentialId, reason: CredentialEnd) {
        let Ok(name) = BusName::try_from(owner.to_owned()) else {
            return;
        };
        let Ok(emitter) = SignalEmitter::new(&self.connection, ACCOUNTS_PATH) else {
            return;
        };
        let _ = emitter
            .set_destination(name)
            .emit(
                TOKENS_INTERFACE,
                "ProcessCredentialRevoked",
                &(id.as_str(), reason.word()),
            )
            .await;
    }
}
