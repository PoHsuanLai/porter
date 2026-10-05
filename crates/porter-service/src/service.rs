//! The service: one request in, one reply out, for a caller the transport has identified.

use crate::audience::covers;
use crate::audit::{AuditSink, NoAudit};
use crate::choose::{ask_for, settle};
use crate::clock::Clock;
use crate::registry::{Asker, Registry};
use crate::sheets::Sheets;
use crate::store::{NoStore, RegistryStore};
use crate::token::{provider_refusal, secret_purpose, secrets_refusal};
use porter_core::audit::{AuditEntry, AuditEvent};
use porter_core::consent::{GrantScope, Usage, availability};
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AccountState, AccountsReply, AccountsRequest, AppId, Audience, DataClass,
    EndpointUrl, GrantId, Need, RelayPlan, SecretKey,
};
use porter_provider::{Presented, Provider, ProviderSession, ProviderSet};
use porter_secrets::{Secrets, SecretsError};
use std::sync::{Mutex, MutexGuard};

/// accountd's core over its seams. The store and the audit sink default to none: the registry
/// then lives for the process, and nothing is logged.
#[derive(Debug)]
pub struct AccountService<P, S, U, K, R = NoStore, A = NoAudit> {
    pub(crate) providers: Vec<P>,
    pub(crate) catalog: ProviderSet,
    pub(crate) secrets: S,
    pub(crate) sheets: U,
    pub(crate) clock: K,
    pub(crate) store: R,
    pub(crate) audit: A,
    pub(crate) registry: Mutex<Registry>,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock> AccountService<P, S, U, K> {
    /// A service over these providers, starting from `registry` (loaded by the host).
    pub fn new(providers: Vec<P>, registry: Registry, secrets: S, sheets: U, clock: K) -> Self {
        let specs = providers.iter().map(|p| p.spec().clone()).collect();
        let catalog = ProviderSet::layered(specs, Vec::new());
        Self {
            providers,
            catalog,
            secrets,
            sheets,
            clock,
            store: NoStore,
            audit: NoAudit,
            registry: Mutex::new(registry),
        }
    }
}

impl<P, S, U, K, R, A> AccountService<P, S, U, K, R, A> {
    /// The same service saving its registry to `store` after every change.
    pub fn with_store<N: RegistryStore>(self, store: N) -> AccountService<P, S, U, K, N, A> {
        AccountService {
            providers: self.providers,
            catalog: self.catalog,
            secrets: self.secrets,
            sheets: self.sheets,
            clock: self.clock,
            store,
            audit: self.audit,
            registry: self.registry,
        }
    }

    /// The same service recording its decisions to `audit`.
    pub fn with_audit<N: AuditSink>(self, audit: N) -> AccountService<P, S, U, K, R, N> {
        AccountService {
            providers: self.providers,
            catalog: self.catalog,
            secrets: self.secrets,
            sheets: self.sheets,
            clock: self.clock,
            store: self.store,
            audit,
            registry: self.registry,
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Registry> {
        // Every critical section is a plain data update that cannot panic midway.
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// A copy of the registry, for the host to persist and for the Settings module.
    pub fn registry(&self) -> Registry {
        self.lock().clone()
    }

    /// Answers `request` from `caller`. `OpenAuthenticated` carries a descriptor the transport
    /// makes, so it is answered by [`AccountService::open_authenticated`] and is `Unavailable`
    /// here.
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
            AccountsRequest::AddAccount { hint, window } => {
                self.add_account(caller, hint, window).await
            }
            AccountsRequest::Reauthenticate { account, window } => {
                self.reauthenticate(caller, &account, window).await
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
            AccountsRequest::Revoke { grant } => self.revoke(caller, &grant).await,
            AccountsRequest::IssueToken { grant, audience } => {
                self.issue_token(caller, &grant, &audience).await
            }
            AccountsRequest::OpenAuthenticated { .. } => {
                AccountsReply::Refused(Refusal::Unavailable)
            }
            AccountsRequest::Adopt { legacy } => self.adopt(caller, legacy).await,
        }
    }

    /// What the relay for `OpenAuthenticated` is told: checks that `caller` holds the grant and
    /// that `endpoint` is one of the account's for the grant's kind, then reads what the relay
    /// presents. The transport makes the descriptor and runs the relay.
    pub async fn open_authenticated(
        &self,
        caller: &AppId,
        grant: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        let (account, endpoint, kind) = {
            let registry = self.lock();
            let target = registry.relay_target(caller, grant, endpoint)?;
            (target.account.clone(), target.endpoint.clone(), target.kind)
        };
        if account.state == AccountState::NeedsReauth {
            return Err(Refusal::NeedsReauth);
        }
        self.relay_plan(&account, &endpoint, kind).await
    }

    /// Removes an account and everything filed for it: secrets, grants, toggles, the registry
    /// row.
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        self.secrets.delete_account(id).await?;
        {
            let mut registry = self.lock();
            registry.accounts.retain(|a| a.id != *id);
            registry.grants.retain(|g| g.key.account != *id);
            registry.toggles.retain(|t| t.account != *id);
        }
        self.note(None, Some(id.clone()), AuditEvent::Removed);
        // The in-memory registry is already without the account; a store that cannot be
        // written keeps the old row until the next save, and the host sees it on its own save.
        let _ = self.persist().await;
        Ok(())
    }

    /// Saves the registry; `Unavailable` when the store cannot be written. The change stays in
    /// memory, and the next save writes it.
    pub(crate) async fn persist(&self) -> Result<(), Refusal> {
        let snapshot = self.lock().persisted();
        self.store
            .save(&snapshot)
            .await
            .map_err(|_| Refusal::Unavailable)
    }

    /// Records one audited event, dated now.
    pub(crate) fn note(&self, app: Option<AppId>, account: Option<AccountId>, event: AuditEvent) {
        self.audit.record(AuditEntry {
            at: self.clock.now(),
            app,
            account,
            event,
        });
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
        let answer = self.sheets.consent(ask, window).await;
        let reply = settle(&mut self.lock(), &need, asker, answer, self.clock.now());
        match &reply {
            AccountsReply::Chosen(candidate) => self.note(
                Some(caller.clone()),
                Some(candidate.account.clone()),
                AuditEvent::Granted {
                    grant: candidate.grant.clone(),
                    kind: need.kind(),
                },
            ),
            AccountsReply::Refused(Refusal::Denied) => self.note(
                Some(caller.clone()),
                None,
                AuditEvent::Denied { kind: need.kind() },
            ),
            _ => return reply,
        }
        match self.persist().await {
            Ok(()) => reply,
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    async fn revoke(&self, caller: &AppId, grant: &GrantId) -> AccountsReply {
        let account = {
            let mut registry = self.lock();
            let Some(held) = registry.grant_of(caller, grant) else {
                return AccountsReply::Refused(Refusal::UnknownGrant);
            };
            let account = held.key.account.clone();
            registry.grants.retain(|g| g.id != *grant);
            account
        };
        self.note(
            Some(caller.clone()),
            Some(account),
            AuditEvent::Revoked {
                grant: grant.clone(),
            },
        );
        match self.persist().await {
            Ok(()) => AccountsReply::Revoked,
            Err(refusal) => AccountsReply::Refused(refusal),
        }
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
                Some((account.clone(), g.scope, g.decision, g.key.kind))
            })
        };
        let Some((account, scope, porter_core::consent::Decision::Allow, kind)) = found else {
            return AccountsReply::Refused(Refusal::UnknownGrant);
        };
        if account.state == AccountState::NeedsReauth {
            return AccountsReply::Refused(Refusal::NeedsReauth);
        }
        let Some(spec) = self.catalog.get(&account.provider) else {
            return AccountsReply::Refused(Refusal::Unavailable);
        };
        if !covers(spec, kind, audience) {
            return AccountsReply::Refused(Refusal::AudienceNotGranted);
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
            if let Err(refusal) = self.persist().await {
                return AccountsReply::Refused(refusal);
            }
        }
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::TokenIssued {
                grant: grant.clone(),
                audience: audience.clone(),
            },
        );
        AccountsReply::Token(token)
    }
}
