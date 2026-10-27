//! What the launcher asks of the key host: [`LauncherHost`], the seam between the bus object and
//! the hosted `porter_service::AccountService` that holds the AI keys and the permits.
//!
//! The service is the implementation for every choice of its seam parameters, as accountd's `Host`
//! was. The trait keeps the bus object free of those six parameters, and lets a test host stand in
//! for the service.

use porter_core::clock::Clock;
use porter_core::consent::Usage;
use porter_core::store::RegistryStore;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AccountsReply, AgentState, AppId, DataClass, GrantId, LauncherSession, Need,
    SecretText,
};
use porter_provider::Provider;
use porter_secrets::Secrets;
use porter_service::{AccountService, AgentFault, AuditSink, Registry, Roster, Sheets};
use std::future::Future;

/// The key host as the launcher uses it: its accounts and permits, the consent sheet for an
/// agent's key, and the keys themselves.
pub trait LauncherHost: Send + Sync + 'static {
    /// A copy of the registry: the accounts, and the permits (the grants of kind Llm).
    fn registry(&self) -> Registry;

    /// Tells the host which agent programs have a launcher, so that adding an agent account with
    /// none ends in `NoLauncher`. A host that adds no agent accounts ignores it.
    fn use_launcher_roster(&self, roster: Roster) {
        let _ = roster;
    }

    /// Records what an agent program says of its own sign-in; whether the account's state
    /// changed.
    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send;

    /// The consent sheet for an agent program's key: `agent` is the app
    /// `org.quire.Agent.<program>` the permit is held under, and `session` an open launcher
    /// session of the caller, which lets the sheet offer "This session only".
    fn choose_for_agent(
        &self,
        agent: &AppId,
        need: Need,
        class: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Removes the permits scoped to `session` (every session permit when none is named) and
    /// returns them.
    fn end_session_grants(
        &self,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = Vec<GrantId>> + Send;

    /// The API key filed for `account`. A key that is gone or unreadable means signing in again
    /// (`NeedsReauth`); a store that cannot be reached is `Unavailable`.
    fn read_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send;
}

impl<P, S, U, K, R, A> LauncherHost for AccountService<P, S, U, K, R, A>
where
    P: Provider + 'static,
    S: Secrets + 'static,
    U: Sheets + 'static,
    K: Clock + 'static,
    R: RegistryStore + 'static,
    A: AuditSink + 'static,
{
    fn registry(&self) -> Registry {
        AccountService::registry(self)
    }

    fn use_launcher_roster(&self, roster: Roster) {
        AccountService::set_launcher_roster(self, roster);
    }

    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send {
        AccountService::set_agent_state(self, account, state)
    }

    fn choose_for_agent(
        &self,
        agent: &AppId,
        need: Need,
        class: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::choose_for_agent(self, agent, need, class, window, session)
    }

    fn end_session_grants(
        &self,
        session: Option<&LauncherSession>,
    ) -> impl Future<Output = Vec<GrantId>> + Send {
        AccountService::end_session_grants(self, session)
    }

    // The one call here that is not in porter-service yet: `AccountService::api_key(&self,
    // &AccountId) -> Result<SecretText, Refusal>` reads the account's `SecretPurpose::ApiKey`
    // from the service's own secrets (`Credential::ApiKey`; `Missing` and `Unreadable` are
    // `NeedsReauth`, `Locked` and `Unavailable` are `Unavailable`, as accountd's `KeyDesk` mapped
    // them). It is a few lines beside `issue_for`, which already reads the same secret.
    fn read_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send {
        AccountService::api_key(self, account)
    }
}
