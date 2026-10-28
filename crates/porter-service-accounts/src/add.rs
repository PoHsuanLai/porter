//! The entry points that start an add, a sign-in again, or a request for more access, and the
//! API key a host that runs the agent launcher reads. The loops they start are `add_flow`'s.

use crate::Accounts;
use crate::add_flow::SignInAs;
use porter_capability::CapabilityKind;
use porter_core::AccountId;
use porter_core::ProviderHint;
use porter_core::{AppId, Credential, Family, OWN_SERVER_PROVIDER, OwnServerRow};
use porter_core::{SecretKey, SecretPurpose, SecretText};
use porter_secrets::Secrets;
use porter_secrets::SecretsError;
use porter_service_core::Ports;
use porter_wire::AccountsReply;
use porter_wire::{ParentWindow, Refusal};

impl<E: Ports> Accounts<'_, E> {
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

    /// See [`AccountsApi::api_key`](crate::AccountsApi::api_key).
    pub(crate) async fn api_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        let key = SecretKey {
            account: account.clone(),
            purpose: SecretPurpose::ApiKey,
        };
        match self.ctx.secrets().get(&key).await {
            Ok(Credential::ApiKey(key)) => Ok(key),
            Ok(_) | Err(SecretsError::Missing | SecretsError::Unreadable) => {
                Err(Refusal::NeedsReauth)
            }
            // locked, unreachable, and a failure a later porter-secrets adds
            Err(_) => Err(Refusal::Unavailable),
        }
    }

    /// See [`AccountsApi::own_servers`](crate::AccountsApi::own_servers).
    pub(crate) fn own_servers(&self) -> Vec<OwnServerRow> {
        self.ctx
            .lock()
            .accounts
            .iter()
            .filter(|account| account.provider.as_str() == OWN_SERVER_PROVIDER)
            .filter_map(|account| {
                let endpoint = account
                    .endpoints
                    .iter()
                    .find(|endpoint| endpoint.family == Family::ChatCompletions)?;
                Some(OwnServerRow::new(
                    account.id.clone(),
                    account.label.0.clone(),
                    endpoint.clone(),
                ))
            })
            .collect()
    }

    /// See [`AccountsApi::own_server_key`](crate::AccountsApi::own_server_key).
    pub(crate) async fn own_server_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        let account = {
            let registry = self.ctx.lock();
            let account = registry
                .accounts
                .iter()
                .find(|a| a.id == *account && a.provider.as_str() == OWN_SERVER_PROVIDER)
                .ok_or(Refusal::UnknownGrant)?
                .clone();
            registry.llm_usable(&account)?;
            account
        };
        self.api_key(&account.id).await
    }

    /// See [`AccountsApi::sign_in_as`](crate::AccountsApi::sign_in_as).
    pub(crate) async fn sign_in_as(
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
        self.run_reauthenticate(caller, account, window, false)
            .await
    }

    /// See [`AccountsApi::reauthenticate_any`](crate::AccountsApi::reauthenticate_any).
    pub(crate) async fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        self.run_reauthenticate(caller, account, window, true).await
    }

    /// See [`AccountsApi::extend_access`](crate::AccountsApi::extend_access).
    pub(crate) async fn extend_access(
        &self,
        caller: &AppId,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> Result<(), Refusal> {
        self.run_extend(caller, account, kinds, window).await
    }
}
