//! The flows that need a family's sign-in or an app's legacy store: add an account, sign one in
//! again, adopt a legacy account, and what an authenticated relay presents. The checks that
//! need no provider are built (`Registry::relay_target`, the audience rule); these bodies wait
//! for the first family.

use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::wire::{LegacyRef, ParentWindow, ProviderHint, Refusal};
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
        _caller: &AppId,
        _hint: ProviderHint,
        _window: ParentWindow,
    ) -> AccountsReply {
        todo!(
            "open the sheet with `Sheet::new`, drive `Provider::sign_in` and `sheet::step` until \
             Done, file the credentials, store the account with its endpoints, audit `SignedIn`"
        )
    }

    /// Runs the provider's sign-in again for an account the caller holds a grant for and files
    /// the new credential under the same id.
    pub(crate) async fn reauthenticate(
        &self,
        _caller: &AppId,
        _account: &AccountId,
        _window: ParentWindow,
    ) -> AccountsReply {
        todo!(
            "open the sheet on the account's provider, drive `Provider::sign_in` with \
             `SignInMode::Reauthenticate`, replace the credential, set the state Ok, audit \
             `Reauthed`"
        )
    }

    /// Reads the app's old secret items (the daemon's `[adopt]` table names the legacy service
    /// for the caller), makes the account and files its credentials.
    pub(crate) async fn adopt(&self, _caller: &AppId, _legacy: LegacyRef) -> AccountsReply {
        todo!(
            "check the caller against the `[adopt]` table, read each `LegacyItem` through the \
             legacy store seam, file it under its `SecretPurpose`, store the account, audit \
             `Adopted`"
        )
    }

    /// What the relay for one checked endpoint presents: the account's password, or an access
    /// token the provider session mints.
    pub(crate) async fn relay_plan(
        &self,
        _account: &Account,
        _endpoint: &ServiceEndpoint,
        _kind: CapabilityKind,
    ) -> Result<RelayPlan, Refusal> {
        todo!(
            "read the credential, mint an access token for an OAuth account, build the \
             `RelayAuth`"
        )
    }
}
