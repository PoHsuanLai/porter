//! The flows that need a family's sign-in or an app's legacy store: add an account, sign one in
//! again, and what an authenticated relay presents. The checks that
//! need no provider are built (`Registry::relay_target`, the audience rule); these bodies wait
//! for the first family.

use crate::add_flow::{Reauth, SignInAs};
use crate::audit::AuditSink;
use crate::service::AccountService;
use crate::sheets::Sheets;
use porter_core::capability::StorageScope;
use porter_core::clock::Clock;
use porter_core::store::RegistryStore;
use porter_core::wire::{ParentWindow, ProviderHint, Refusal};
use porter_core::{
    Account, AccountId, AccountsReply, AppId, CapabilityKind, Credential, ProviderId, RelayPlan,
    SecretKey, SecretPurpose, SecretText, ServiceEndpoint,
};
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};

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
        self.run_add(caller, hint, window, None, None).await
    }

    /// The API key filed for `account`: the in-process read for a host that runs the agent
    /// launcher (inferd). It never crosses a process. A key that is missing or unreadable (or
    /// filed as another kind of credential) means signing in again (`NeedsReauth`); a store that
    /// is locked or unreachable is `Unavailable`.
    pub async fn api_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        let key = SecretKey {
            account: account.clone(),
            purpose: SecretPurpose::ApiKey,
        };
        match self.secrets.get(&key).await {
            Ok(Credential::ApiKey(key)) => Ok(key),
            Ok(_) | Err(SecretsError::Missing | SecretsError::Unreadable) => {
                Err(Refusal::NeedsReauth)
            }
            Err(SecretsError::Locked | SecretsError::Unavailable) => Err(Refusal::Unavailable),
        }
    }

    /// Opens the add-account sheet for an identity the host already knows ("Continue as"): the
    /// provider's page opens on `identity.hint`, and a sign-in that skipped the consent screen
    /// and came back with nothing to keep is started once more as a first sign-in. The sheet
    /// opens on `identity.provider` with no provider list; no grant is recorded.
    pub async fn sign_in_as(
        &self,
        caller: &AppId,
        identity: SignInAs,
        window: ParentWindow,
    ) -> AccountsReply {
        let hint = ProviderHint::Provider(identity.provider.clone());
        self.run_add(caller, hint, window, None, Some(identity))
            .await
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

    /// Asks the person, in a sign-in of its own, for the access `account`'s provider asks only
    /// when it is first used (`ask = "on_use"`) for `kinds`: the whole of a drive, when the
    /// person turns on "Show in Files". The sheet opens on the account, the provider's page asks
    /// for that access alone, and the review names it in plain words; on Allow the new
    /// credential replaces the old under the same id, and the account claims the access given
    /// (the provider's on-use rows of `kinds`), so a need of it fits the account from then on:
    /// an account fits such a need only once the person agreed. An answer from anyone but the account's
    /// own login stores nothing. For the host itself (syncd, for its own accounts): no grant is
    /// asked of `caller`, who is named in the audit.
    ///
    /// # Errors
    /// `UnknownGrant` for an account the registry does not hold; `Unavailable` when its provider
    /// asks none of `kinds` on use, or when what came back could not be kept (another person
    /// included); otherwise the sheet's own ending (`Dismissed`, `Denied` when the person
    /// refused the access).
    pub async fn extend_access(
        &self,
        caller: &AppId,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> Result<(), Refusal> {
        self.run_extend(caller, account, kinds, window).await
    }

    /// Whether `provider` asks some access of `kind` only when it is first used (`ask =
    /// "on_use"`): what [`Self::extend_access`] can ask an account of it for, and so whether a
    /// host offers the switch that asks it ("Show in Files").
    pub fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool {
        self.catalog
            .get(provider)
            .is_some_and(|spec| spec.on_use_rows(kind).next().is_some())
    }

    /// What the relay for one checked endpoint presents: the account's password, or an access
    /// token the provider session mints.
    pub(crate) async fn relay_plan(
        &self,
        account: &Account,
        endpoint: &ServiceEndpoint,
        grant: (CapabilityKind, StorageScope),
    ) -> Result<RelayPlan, Refusal> {
        self.plan_relay(account, endpoint, grant).await
    }
}
