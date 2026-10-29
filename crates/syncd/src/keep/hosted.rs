//! The seams of syncd's own accounts: the service it hosts ([`Hosted`]), the directory of
//! identities it continues as ([`Directory`]) and the sheets it draws ([`SyncSheets`]).

use super::classes::sync_app;
use porter_capability::CapabilityKind;
use porter_client::{Identities, IdentitiesError, IdentitiesLink};
use porter_core::ProviderHint;
use porter_core::consent::{ConsentAnswer, ConsentAsk, Grant, GrantScope};
use porter_core::store::RegistryStore;
use porter_core::{Account, IdentityAddress, IdentityId};
use porter_core::{AccountId, GrantId, ProviderId};
use porter_ids::clock::Clock;
use porter_provider::Provider;
use porter_secrets::{Secrets, SecretsError};
use porter_service::AccountService;
use porter_service_accounts::SignInAs;
use porter_service_core::audit::AuditSink;
use porter_service_core::registry::Registry;
use porter_sheet::host::Sheets;
use porter_wire::{AccountsReply, AccountsRequest};
use porter_wire::{ParentWindow, Refusal};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

/// What syncd needs of the account service it hosts. porter's [`AccountService`] is the one
/// implementation; the trait keeps its six type parameters out of the rest of the crate.
pub trait Hosted: Send + Sync + 'static {
    /// A copy of the registry: the accounts syncd signed in and its own grants.
    fn registry(&self) -> Registry;

    /// One request from syncd itself (`org.quire.Sync`): its own grants are asked for through
    /// this.
    fn answer(&self, request: AccountsRequest) -> impl Future<Output = AccountsReply> + Send;

    /// The grants syncd holds.
    fn grants(&self) -> Vec<Grant>;

    /// Takes back one of syncd's own grants.
    fn revoke(&self, grant: &GrantId) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Signs the person's identity in for syncd, through the sheet, opening the provider's page
    /// on the address.
    fn sign_in_as(
        &self,
        request: SignInAs,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Signs an account in again.
    fn sign_in_again(
        &self,
        account: &AccountId,
        window: ParentWindow,
    ) -> impl Future<Output = AccountsReply> + Send;

    /// Removes an account: its secrets, grants and registry row.
    fn remove_account(
        &self,
        account: &AccountId,
    ) -> impl Future<Output = Result<(), SecretsError>> + Send;

    /// Asks the person, in a sign-in of its own, for the access of `kinds` the account's
    /// provider asks only on use (the whole drive, for "Show in Files"); on Allow the account
    /// claims it. The refusal is the sheet's ending, or `Unavailable`.
    fn extend_access(
        &self,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> impl Future<Output = Result<(), Refusal>> + Send;

    /// Whether `provider` asks some access of `kind` only on use, which
    /// [`Hosted::extend_access`] can ask for.
    fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool;
}

impl<P, S, U, K, R, A> Hosted for AccountService<P, S, U, K, R, A>
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

    async fn answer(&self, request: AccountsRequest) -> AccountsReply {
        self.handle(&sync_app(), request).await
    }

    fn grants(&self) -> Vec<Grant> {
        AccountService::grants(self, &sync_app())
    }

    async fn revoke(&self, grant: &GrantId) -> Result<(), Refusal> {
        AccountService::revoke(self, &sync_app(), grant).await
    }

    async fn sign_in_as(&self, request: SignInAs, window: ParentWindow) -> AccountsReply {
        AccountService::sign_in_as(self, &sync_app(), request, window).await
    }

    async fn sign_in_again(&self, account: &AccountId, window: ParentWindow) -> AccountsReply {
        self.reauthenticate_any(&sync_app(), account, window).await
    }

    async fn remove_account(&self, account: &AccountId) -> Result<(), SecretsError> {
        AccountService::remove_account(self, account).await
    }

    async fn extend_access(
        &self,
        account: &AccountId,
        kinds: Vec<CapabilityKind>,
        window: ParentWindow,
    ) -> Result<(), Refusal> {
        AccountService::extend_access(self, &sync_app(), account, kinds, window).await
    }

    fn asks_on_use(&self, provider: &ProviderId, kind: CapabilityKind) -> bool {
        AccountService::asks_on_use(self, provider, kind)
    }
}

/// The identity the person picked: who syncd continues as. It holds no token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Pick {
    /// The identity's id in accountd, which `register_use` takes.
    pub id: IdentityId,
    /// The address to continue as.
    pub address: IdentityAddress,
    /// The provider's id.
    pub provider: ProviderId,
}

impl Pick {
    /// A pick of this identity.
    pub fn new(id: IdentityId, address: IdentityAddress, provider: ProviderId) -> Self {
        Self {
            id,
            address,
            provider,
        }
    }
}

/// The identities signed in on this computer, as syncd asks for them (accountd's `Identities1`).
pub trait Directory: Send + Sync + 'static {
    /// Shows the person the identity picker and waits for the pick.
    fn offer(
        &self,
        window: &ParentWindow,
    ) -> impl Future<Output = Result<Pick, IdentitiesError>> + Send;

    /// Says that syncd signed in with `id`: it joins the apps Settings lists as using it.
    fn register_use(
        &self,
        id: &IdentityId,
    ) -> impl Future<Output = Result<(), IdentitiesError>> + Send;
}

impl<T: IdentitiesLink> Directory for Identities<T> {
    async fn offer(&self, window: &ParentWindow) -> Result<Pick, IdentitiesError> {
        let picked = Identities::offer(self, &ProviderHint::Any, window).await?;
        Ok(Pick::new(picked.id, picked.address, picked.provider))
    }

    async fn register_use(&self, id: &IdentityId) -> Result<(), IdentitiesError> {
        Identities::register_use(self, id).await
    }
}

/// The account the person is turning sync on for right now: the switch is the consent, so syncd
/// answers its own consent ask from it instead of showing a second sheet.
#[derive(Debug, Clone, Default)]
pub struct Intent(Arc<Mutex<Option<AccountId>>>);

impl Intent {
    /// Says the person turned sync on for `account`.
    pub fn set(&self, account: AccountId) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(account);
    }

    /// Says nothing is being turned on.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn get(&self) -> Option<AccountId> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// syncd's sheets: conversations (sign in) go to `inner`, the sheet host; a consent ask of syncd's
/// own is answered from the [`Intent`] with an always-allow, and is otherwise dismissed. Another
/// app's ask never reaches a service of syncd's.
#[derive(Debug)]
pub struct SyncSheets<S> {
    inner: S,
    intent: Intent,
}

impl<S> SyncSheets<S> {
    /// Sheets over `inner` that read the person's intent from `intent`.
    pub fn new(inner: S, intent: Intent) -> Self {
        Self { inner, intent }
    }
}

impl<S: Sheets> Sheets for SyncSheets<S> {
    type Link = S::Link;

    async fn consent(&self, ask: ConsentAsk, _window: &ParentWindow) -> ConsentAnswer {
        match self.intent.get() {
            Some(account)
                if ask.app == sync_app() && ask.accounts.iter().any(|c| c.account == account) =>
            {
                ConsentAnswer::Allow {
                    account,
                    scope: GrantScope::Always,
                }
            }
            _ => ConsentAnswer::Dismissed,
        }
    }

    fn conversation(
        &self,
        open: porter_sheet::host::SheetOpen,
    ) -> impl Future<Output = Result<Self::Link, porter_sheet::host::SheetFault>> + Send {
        self.inner.conversation(open)
    }
}

/// An account of syncd's registry by its id.
pub fn account_of(registry: &Registry, id: &AccountId) -> Option<Account> {
    registry.accounts.iter().find(|a| a.id == *id).cloned()
}
