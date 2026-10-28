//! The entry points that start an add, a sign-in again, or a request for more access, and the
//! API key a host that runs the agent launcher reads. The loops they start are `add_flow`'s.

use crate::add_flow::{Reauth, SignInAs};
use crate::audit::AuditSink;
use crate::ctx::Ctx;
use crate::sheets::Sheets;
use porter_core::AccountId;
use porter_core::ProviderHint;
use porter_core::store::RegistryStore;
use porter_core::{AppId, CapabilityKind, Credential, SecretKey, SecretPurpose, SecretText};
use porter_ids::clock::Clock;
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_wire::AccountsReply;
use porter_wire::{ParentWindow, Refusal};

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    Ctx<P, S, U, K, R, A>
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

    /// The body of [`AccountService::api_key`](crate::AccountService::api_key).
    pub(crate) async fn api_key(&self, account: &AccountId) -> Result<SecretText, Refusal> {
        let key = SecretKey {
            account: account.clone(),
            purpose: SecretPurpose::ApiKey,
        };
        match self.secrets.get(&key).await {
            Ok(Credential::ApiKey(key)) => Ok(key),
            Ok(_) | Err(SecretsError::Missing | SecretsError::Unreadable) => {
                Err(Refusal::NeedsReauth)
            }
            // locked, unreachable, and a failure a later porter-secrets adds
            Err(_) => Err(Refusal::Unavailable),
        }
    }

    /// The body of [`AccountService::sign_in_as`](crate::AccountService::sign_in_as).
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
        self.run_reauthenticate(caller, account, window, Reauth::Held)
            .await
    }

    /// The body of [`AccountService::reauthenticate_any`](crate::AccountService::reauthenticate_any).
    pub(crate) async fn reauthenticate_any(
        &self,
        caller: &AppId,
        account: &AccountId,
        window: ParentWindow,
    ) -> AccountsReply {
        self.run_reauthenticate(caller, account, window, Reauth::Shell)
            .await
    }

    /// The body of [`AccountService::extend_access`](crate::AccountService::extend_access).
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
