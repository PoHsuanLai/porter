//! The service as the bus objects use it: one request in, one reply out, for a caller already
//! identified. [`AccountService`] is the implementation.

use porter_core::clock::Clock;
use porter_core::store::RegistryStore;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{AccountId, AccountState, AccountsReply, AccountsRequest, AppId};
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_service::{AccountService, AuditSink, Registry, RevokeReport, Sheets};
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
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send {
        let _ = id;
        async { Err(SecretsError::Unavailable) }
    }

    /// Records what Settings changed outside the registry (a client id). A host with no audit
    /// records nothing.
    fn audit_settings(&self, event: porter_service::audit::AuditEvent) {
        let _ = event;
    }

    /// Records what `app` did that concerns no account (a desktop-wide Space made, renamed or
    /// removed). A host with no audit records nothing.
    fn audit_app(&self, app: &AppId, event: porter_service::audit::AuditEvent) {
        let _ = (app, event);
    }

    /// Sets an account's state; whether it changed.
    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        let _ = (id, state);
        async { false }
    }
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
    ) -> impl Future<Output = Result<RevokeReport, SecretsError>> + Send {
        AccountService::remove_with_revoke(self, id)
    }

    fn audit_settings(&self, event: porter_service::audit::AuditEvent) {
        AccountService::audit_settings(self, event);
    }

    fn audit_app(&self, app: &AppId, event: porter_service::audit::AuditEvent) {
        AccountService::audit_app(self, app.clone(), event);
    }

    fn set_state(&self, id: &AccountId, state: AccountState) -> impl Future<Output = bool> + Send {
        AccountService::set_state(self, id, state)
    }
}
