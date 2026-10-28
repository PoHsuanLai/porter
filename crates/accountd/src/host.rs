//! The service as the bus objects use it: one request in, one reply out, for a caller already
//! identified. [`AccountService`] is the implementation.

use porter_capability::{CapabilityKind, Claim, Toggle};
use porter_core::consent::Usage;
use porter_core::store::RegistryStore;
use porter_core::{AccountId, GrantId, LlmVerdict, OwnServerRow, ProviderId, SecretText};
use porter_core::{AccountState, AgentState, AppId, Candidate, DataClass, SessionToken};
use porter_ids::clock::Clock;
use porter_match::Need;
use porter_provider::{Provider, RevokeOutcome};
use porter_secrets::{Secrets, SecretsError};
use porter_service::AccountService;
use porter_service_accounts::{AgentFault, Launchers, LocalFault, Roster};
use porter_service_core::audit::AuditSink;
use porter_service_core::registry::Registry;
use porter_service_core::sheets::Sheets;
use porter_wire::{AccountsReply, AccountsRequest};
use porter_wire::{ParentWindow, Refusal};
use std::future::Future;

/// The service as the bus objects use it: one request in, one reply out, for a caller already
/// identified. [`AccountService`] is the implementation; the trait keeps the zbus objects free
/// of its four seam parameters.
pub trait Host: Send + Sync + 'static {
    /// Answers `request` from `caller`.
    fn handle(
        &self,
        caller: &AppId,
        request: AccountsRequest,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// A copy of the registry, for the objects that mirror accounts.
    fn registry(&self) -> Registry;

    /// Signs any account in again for the sheet host or Settings, which need no grant (accountd
    /// checks the role). A host with no sign-in says unavailable.
    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        let _ = (caller, account, window);
        async { AccountsReply::Refused(Refusal::Unavailable) }
    }

    /// Removes an account: revoke at the provider (best effort), then every wipe. A host that
    /// cannot manage accounts says its secret store is unavailable.
    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<Option<RevokeOutcome>, SecretsError>> + Send {
        let _ = id;
        async { Err(SecretsError::Unavailable) }
    }

    /// Records what Settings changed outside the registry (a client id). A host with no audit
    /// records nothing.
    fn audit_settings(&self, event: porter_service_core::audit::AuditEvent) {
        let _ = event;
    }

    /// Records what `app` did that concerns no account (a desktop-wide Space made, renamed or
    /// removed). A host with no audit records nothing.
    fn audit_app(&self, app: &AppId, event: porter_service_core::audit::AuditEvent) {
        let _ = (app, event);
    }

    /// Sets an account's state; whether it changed.
    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        let _ = (id, state);
        async { false }
    }

    /// Switches one kind of one account on or off.
    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Withdraws any grant, whoever holds it (Settings' "Revoke").
    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Shows the chooser and consent sheet for `need` and records the grant the person gives;
    /// `session` is the launcher session the ask was made in, which lets the sheet offer "This
    /// session only".
    fn choose(
        &self,
        caller: &AppId,
        need: Need,
        class_usage: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// The permit `app` already holds for `need`, the newest, so a repeat request is answered
    /// with no sheet.
    fn existing_permit(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Option<Candidate>;

    /// What each account that serves language models says for `app`.
    fn llm_verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
        provider_label: &dyn Fn(&ProviderId) -> Option<String>,
    ) -> Vec<LlmVerdict>;

    /// The key behind an allow permit of the language-model kind, read once.
    fn release_key(
        &self,
        grant: &GrantId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send;

    /// The API key filed for `account`.
    fn api_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send;

    /// The accounts that are servers of the person's own.
    fn own_servers(&self) -> Vec<OwnServerRow>;

    /// The key of the own server `account`.
    fn own_server_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send;

    /// Removes the permits scoped to `session` (every session permit when none is named) and
    /// returns them.
    fn end_session_grants(
        &self,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = Vec<GrantId>> + Send;

    /// Records what an agent program says of its own sign-in; whether the account's state
    /// changed.
    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send;

    /// Signs an agent account in through its launcher, with a sheet that shows the wait.
    fn login_agent_any<L: Launchers>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Takes a local runtime's report: the account of `provider`, its models, its state.
    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send;

    /// Tells the host which agent programs have a launcher, so that adding an agent account
    /// with none ends in `NoLauncher`.
    fn use_launcher_roster(&self, roster: Roster);
}

impl<P, S, U, K, R, A> Host for AccountService<P, S, U, K, R, A>
where
    P: Provider + 'static,
    S: Secrets + 'static,
    U: Sheets + 'static,
    K: Clock + 'static,
    R: RegistryStore + 'static,
    A: AuditSink + 'static,
{
    fn handle(
        &self,
        caller: &AppId,
        request: AccountsRequest,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::handle(self, caller, request)
    }

    fn registry(&self) -> Registry {
        AccountService::registry(self)
    }

    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::reauthenticate_any(self, caller, account, window)
    }

    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<Option<RevokeOutcome>, SecretsError>> + Send {
        AccountService::remove_with_revoke(self, id)
    }

    fn audit_settings(&self, event: porter_service_core::audit::AuditEvent) {
        AccountService::audit_settings(self, event);
    }

    fn audit_app(&self, app: &AppId, event: porter_service_core::audit::AuditEvent) {
        AccountService::audit_app(self, app.clone(), event);
    }

    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        AccountService::set_state(self, id, state)
    }

    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::set_toggle(self, id, kind, toggle)
    }

    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::revoke_grant(self, grant)
    }

    fn choose(
        &self,
        caller: &AppId,
        need: Need,
        class_usage: (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::choose(self, caller, need, class_usage, window, session)
    }

    fn existing_permit(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Option<Candidate> {
        AccountService::existing_permit(self, app, need, class, usage)
    }

    fn llm_verdicts(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
        provider_label: &dyn Fn(&ProviderId) -> Option<String>,
    ) -> Vec<LlmVerdict> {
        AccountService::llm_verdicts(self, app, class, usage, provider_label)
    }

    fn release_key(
        &self,
        grant: &GrantId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send {
        AccountService::release_key(self, grant)
    }

    fn api_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send {
        AccountService::api_key(self, account)
    }

    fn own_servers(&self) -> Vec<OwnServerRow> {
        AccountService::own_servers(self)
    }

    fn own_server_key(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<SecretText, Refusal>> + Send {
        AccountService::own_server_key(self, account)
    }

    fn end_session_grants(
        &self,
        session: Option<&SessionToken>,
    ) -> impl Future<Output = Vec<GrantId>> + Send {
        AccountService::end_session_grants(self, session)
    }

    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send {
        AccountService::set_agent_state(self, account, state)
    }

    fn login_agent_any<L: Launchers>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::login_agent_any(self, caller, account, window, launchers)
    }

    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send {
        AccountService::report_local(self, provider, models, state)
    }

    fn use_launcher_roster(&self, roster: Roster) {
        AccountService::set_launcher_roster(self, roster);
    }
}
