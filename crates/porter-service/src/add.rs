//! The flows that need a family's sign-in or an app's legacy store: add an account, sign one in
//! again, and what an authenticated relay presents. The checks that
//! need no provider are built (`Registry::relay_target`, the audience rule); these bodies wait
//! for the first family.

use crate::add_flow::Reauth;
use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    Account, AccountId, AccountsReply, AppId, CapabilityKind, RelayPlan, ServiceEndpoint,
};
use porter_provider::Provider;
use porter_secrets::Secrets;

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Opens the add-account sheet, drives the provider's sign-in through it, discovers, stores
    /// the account and, when the sheet was opened by an app's chooser, its first grant.
    pub(crate) async fn add_account(
        &self,
        caller: &AppId,
        hint: ProviderHint,
        window: ParentWindow,
    ) -> AccountsReply {
        self.run_add(caller, hint, window, None).await
    }

    /// Runs the provider's sign-in again for an account the caller holds a grant for and files
    /// the new credential under the same id.
    pub(crate) async fn reauthenticate(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        self.run_reauthenticate(caller, account, window, Reauth::Held)
            .await
    }

    /// Runs the sign-in again for any account, with no grant: for the sheet host and Settings.
    pub async fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        self.run_reauthenticate(caller, account, window, Reauth::Shell)
            .await
    }

    /// What the relay for one checked endpoint presents: the account's password, or an access
    /// token the provider session mints.
    pub(crate) async fn relay_plan(
        &self,
        account: &Account,
        endpoint: &ServiceEndpoint,
        kind: CapabilityKind,
    ) -> Result<RelayPlan, Refusal> {
        self.plan_relay(account, endpoint, kind).await
    }
}
