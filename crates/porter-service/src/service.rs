//! The service: one request in, one reply out, for a caller the transport has identified.

use crate::choose::{ask_for, settle};
use crate::clock::Clock;
use crate::prompter::Prompter;
use crate::registry::{Asker, Registry};
use crate::token::{provider_refusal, secret_purpose, secrets_refusal};
use porter_core::consent::{GrantScope, Usage, availability};
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AccountState, AccountsReply, AccountsRequest, AppId, Audience, DataClass, GrantId,
    Need, SecretKey,
};
use porter_provider::{Presented, Provider, ProviderSession, ProviderSet};
use porter_secrets::{Secrets, SecretsError};
use std::sync::{Mutex, MutexGuard};

/// accountd's core over its seams.
#[derive(Debug)]
pub struct AccountService<P, S, U, K> {
    providers: Vec<P>,
    catalog: ProviderSet,
    secrets: S,
    prompter: U,
    clock: K,
    registry: Mutex<Registry>,
}

impl<P: Provider, S: Secrets, U: Prompter, K: Clock> AccountService<P, S, U, K> {
    /// A service over these providers, starting from `registry` (loaded by the host).
    pub fn new(providers: Vec<P>, registry: Registry, secrets: S, prompter: U, clock: K) -> Self {
        let specs = providers.iter().map(|p| p.spec().clone()).collect();
        let catalog = ProviderSet::layered(specs, Vec::new());
        Self {
            providers,
            catalog,
            secrets,
            prompter,
            clock,
            registry: Mutex::new(registry),
        }
    }

    /// A copy of the registry, for the host to persist and for the Settings module.
    pub fn registry(&self) -> Registry {
        self.lock().clone()
    }

    /// Answers `request` from `caller`.
    pub async fn handle(&self, caller: &AppId, request: AccountsRequest) -> AccountsReply {
        match request {
            AccountsRequest::Query { need, class, usage } => {
                let asker = Asker {
                    app: caller,
                    class,
                    usage,
                };
                AccountsReply::Candidates(self.lock().candidates(&need, asker))
            }
            AccountsRequest::Availability { need, class, usage } => {
                let asker = Asker {
                    app: caller,
                    class,
                    usage,
                };
                let verdicts = self.lock().verdicts(&need, asker);
                AccountsReply::Availability(availability(&verdicts, self.catalog.catalog(&need)))
            }
            AccountsRequest::Choose {
                need,
                class,
                usage,
                window,
            } => self.choose(caller, need, (class, usage), &window).await,
            AccountsRequest::AddAccount { .. } => {
                todo!("open accounts-ui's add sheet, run the provider's sign-in, discover, store")
            }
            AccountsRequest::Reauthenticate { .. } => {
                todo!("run the account's sign-in again and store the new credential")
            }
            AccountsRequest::ListGrants => {
                let grants = self
                    .lock()
                    .grants
                    .iter()
                    .filter(|g| g.key.app == *caller)
                    .cloned()
                    .collect();
                AccountsReply::Grants(grants)
            }
            AccountsRequest::Revoke { grant } => self.revoke(caller, &grant),
            AccountsRequest::IssueToken { grant, audience } => {
                self.issue_token(caller, &grant, &audience).await
            }
        }
    }

    /// Removes an account and everything filed for it: secrets, grants, the registry row.
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        self.secrets.delete_account(id).await?;
        let mut registry = self.lock();
        registry.accounts.retain(|a| a.id != *id);
        registry.grants.retain(|g| g.key.account != *id);
        Ok(())
    }

    async fn choose(
        &self,
        caller: &AppId,
        need: Need,
        (class, usage): (DataClass, Usage),
        window: &ParentWindow,
    ) -> AccountsReply {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        let Some(ask) = ask_for(&self.lock(), &need, asker) else {
            return AccountsReply::Refused(Refusal::NoFittingAccount);
        };
        let answer = self.prompter.ask(ask, window).await;
        settle(&mut self.lock(), &need, asker, answer, self.clock.now())
    }

    fn revoke(&self, caller: &AppId, grant: &GrantId) -> AccountsReply {
        let mut registry = self.lock();
        if registry.grant_of(caller, grant).is_none() {
            return AccountsReply::Refused(Refusal::UnknownGrant);
        }
        registry.grants.retain(|g| g.id != *grant);
        AccountsReply::Revoked
    }

    async fn issue_token(
        &self,
        caller: &AppId,
        grant: &GrantId,
        audience: &Audience,
    ) -> AccountsReply {
        let found = {
            let registry = self.lock();
            registry.grant_of(caller, grant).and_then(|g| {
                let account = registry.accounts.iter().find(|a| a.id == g.key.account)?;
                Some((account.clone(), g.scope, g.decision))
            })
        };
        let Some((account, scope, porter_core::consent::Decision::Allow)) = found else {
            return AccountsReply::Refused(Refusal::UnknownGrant);
        };
        if account.state == AccountState::NeedsReauth {
            return AccountsReply::Refused(Refusal::NeedsReauth);
        }
        let Some(provider) = self
            .providers
            .iter()
            .find(|p| p.spec().id == account.provider)
        else {
            return AccountsReply::Refused(Refusal::Unavailable);
        };
        let presented = match secret_purpose(account.auth) {
            None => Presented::Anonymous,
            Some(purpose) => {
                let key = SecretKey {
                    account: account.id.clone(),
                    purpose,
                };
                match self.secrets.get(&key).await {
                    Ok(credential) => Presented::Credential(credential),
                    Err(error) => return AccountsReply::Refused(secrets_refusal(error)),
                }
            }
        };
        let session = match provider.open(&account.id, presented).await {
            Ok(session) => session,
            Err(error) => return AccountsReply::Refused(provider_refusal(error)),
        };
        let token = match session.access_token(audience).await {
            Ok(token) => token,
            Err(error) => return AccountsReply::Refused(provider_refusal(error)),
        };
        if let (Some(renewed), Some(purpose)) = (session.renewed(), secret_purpose(account.auth)) {
            let key = SecretKey {
                account: account.id.clone(),
                purpose,
            };
            if let Err(error) = self.secrets.put(&key, &renewed).await {
                return AccountsReply::Refused(secrets_refusal(error));
            }
        }
        if scope == GrantScope::Once {
            self.lock().grants.retain(|g| g.id != *grant);
        }
        AccountsReply::Token(token)
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        // Every critical section is a plain data update that cannot panic midway.
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
