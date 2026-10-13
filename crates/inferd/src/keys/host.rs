//! The hosted service as the rest of inferd uses it: one request in, one reply out, plus the
//! few things Settings and the local-runtime report do to it. [`AccountService`] is the
//! implementation; the trait keeps the bus objects free of its six type parameters.

use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AccountState, AccountsReply, AccountsRequest, AgentState, AppId, CapabilityKind,
    Claim, GrantId, ProviderId, Toggle,
};
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_service::{
    AccountService, AgentFault, AuditSink, Clock, Launchers as LaunchersSeam, LocalFault, Registry,
    RegistryStore, RevokeReport, Sheets,
};
use std::future::Future;

/// The service as the key objects use it.
pub trait KeyHost: Send + Sync + 'static {
    /// Answers `request` from `caller`.
    fn handle(
        &self,
        caller: &AppId,
        request: AccountsRequest,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// A copy of the registry.
    fn registry(&self) -> Registry;

    /// Spends `caller`'s permit when it is an allow-once one; any other scope is left as it is.
    fn spend_once(
        &self,
        caller: &AppId,
        grant: &GrantId,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Signs any account in again for Settings, which needs no permit.
    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Signs an agent account in through its launcher, with a sheet that shows the wait.
    fn login_agent_any<L: LaunchersSeam>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Removes an account: revoke at the provider (best effort), then every wipe.
    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send;

    /// Switches one kind of one account on or off.
    fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Withdraws any permit, whoever holds it.
    fn revoke_grant(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Records what an agent program says of its own sign-in.
    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send;

    /// Takes a local runtime's report: the account of `provider`, its models, its state.
    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send;
}

impl<P, S, U, K, R, A> KeyHost for AccountService<P, S, U, K, R, A>
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

    fn spend_once(
        &self,
        caller: &AppId,
        grant: &GrantId,
    ) -> impl Future<Output = Result<(), Refusal>> + Send {
        AccountService::spend_once(self, caller, grant)
    }

    fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::reauthenticate_any(self, caller, account, window)
    }

    fn login_agent_any<L: LaunchersSeam>(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
        launchers: &L,
    ) -> impl Future<Output = AccountsReply> + Send {
        AccountService::login_agent_any(self, caller, account, window, launchers)
    }

    fn remove(
        &self,
        id: &AccountId,
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send {
        AccountService::remove_with_revoke(self, id)
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

    fn set_agent_state(
        &self,
        account: &AccountId,
        state: AgentState,
    ) -> impl Future<Output = Result<bool, AgentFault>> + Send {
        AccountService::set_agent_state(self, account, state)
    }

    fn report_local(
        &self,
        provider: &ProviderId,
        models: Vec<Claim>,
        state: AccountState,
    ) -> impl Future<Output = Result<AccountId, LocalFault>> + Send {
        AccountService::report_local(self, provider, models, state)
    }
}
