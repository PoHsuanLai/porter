//! The service: one request in, one reply out, for a caller the transport has identified.

use crate::agent_login::Roster;
use crate::audience::covers;
use crate::audit::{AuditSink, NoAudit};
use crate::choose::{ask_for, settle};
use crate::clock::Clock;
use crate::registry::{Asker, Registry};
use crate::sheets::Sheets;
use crate::store::{NoStore, RegistryStore};
use crate::token::{provider_refusal, secret_purpose, secrets_refusal};
use porter_core::audit::{AuditEntry, AuditEvent};
use porter_core::consent::{ConsentAnswer, GrantScope, Usage, availability};
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{
    AccountId, AccountState, AccountsReply, AccountsRequest, AppId, Audience, AuthKind, DataClass,
    EndpointUrl, GrantId, LauncherSession, Need, ProviderId, RelayPlan, SecretKey,
};
use porter_provider::{
    Presented, Provider, ProviderError, ProviderSession, ProviderSet, ProviderSpec,
};
use porter_secrets::{Secrets, SecretsError};
use std::sync::{Mutex, MutexGuard, OnceLock};

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
    /// Which agent programs have a launcher, once the host says (accountd, when it serves).
    pub(crate) roster: OnceLock<Roster>,
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
            roster: OnceLock::new(),
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
            roster: self.roster,
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
            roster: self.roster,
        }
    }

    /// Tells the service which agent programs have a launcher, so that adding an agent account
    /// with none ends in `SignInFault::NoLauncher` before anything is stored. Set once; a
    /// service that is never told (no launcher bus: an app hosting porter in process) adds
    /// agent accounts as it always did.
    pub fn set_launcher_roster(&self, roster: Roster) {
        let _ = self.roster.set(roster);
    }

    /// The same service knowing the local runtimes (`AuthKind::LocalRuntime`) among `specs`:
    /// they have no family to sign in through, but `Peer.ReportLocal` makes their accounts and
    /// the catalogue lists them. Any other spec is ignored (a provider needs its family).
    #[must_use]
    pub fn with_local_runtimes(mut self, specs: Vec<ProviderSpec>) -> Self {
        let mut all = self.catalog.specs().to_vec();
        let known = |all: &[ProviderSpec], id: &ProviderId| all.iter().any(|s| s.id == *id);
        for spec in specs {
            if spec.auth.kind == AuthKind::LocalRuntime && !known(&all, &spec.id) {
                all.push(spec);
            }
        }
        self.catalog = ProviderSet::layered(all, Vec::new());
        self
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
            } => {
                self.choose(caller, need, (class, usage), &window, None)
                    .await
            }
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
            AccountsRequest::OpenAuthenticated { .. } | AccountsRequest::OpenLinked { .. } => {
                AccountsReply::Refused(Refusal::Unavailable)
            }
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
            match registry.relay_target(caller, grant, endpoint) {
                Ok(target) => (target.account.clone(), target.endpoint.clone(), target.kind),
                // Not one of the account's endpoints: perhaps an origin its provider file names
                // as one that wants the bearer (picked photos' bytes).
                Err(Refusal::EndpointNotGranted) => {
                    self.auth_origin_target(&registry, caller, grant, endpoint)?
                }
                Err(other) => return Err(other),
            }
        };
        if account.state == AccountState::NeedsReauth {
            return Err(Refusal::NeedsReauth);
        }
        let plan = self.relay_plan(&account, &endpoint, kind).await?;
        self.spend_once(caller, grant).await?;
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::ProxyOpened {
                grant: grant.clone(),
                endpoint: endpoint.url.clone(),
            },
        );
        Ok(plan)
    }

    /// What the relay for `OpenLinked` is told: checks that `caller` holds the grant and that
    /// `origin` is one its account's provider file declares for the grant's kind, then plans a
    /// relay that adds no credential. The audit line names the origin, never a link's path (a
    /// pre-authenticated URL carries its secret there).
    pub async fn open_linked(
        &self,
        caller: &AppId,
        grant: &GrantId,
        origin: &EndpointUrl,
    ) -> Result<RelayPlan, Refusal> {
        let (plan, account) = self.plan_linked(caller, grant, origin)?;
        self.spend_once(caller, grant).await?;
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::ProxyOpened {
                grant: grant.clone(),
                endpoint: plan.endpoint.url.clone(),
            },
        );
        Ok(plan)
    }

    /// Removes an account and everything filed for it: its provider is asked to stop honouring
    /// the credential first (best effort, `remove_with_revoke` says what it answered), then the
    /// secrets, grants, toggles and the registry row go.
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        self.remove_with_revoke(id).await.map(|_| ())
    }

    /// The wipe of `remove_account`: secrets, grants, toggles, the registry row, one audit line.
    pub(crate) async fn wipe_account(&self, id: &AccountId) -> Result<(), SecretsError> {
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

    /// The consent sheet for an agent program's key (`Peer.RequestAgentGrant`): `agent` is the
    /// app `org.quire.Agent.<program>`, the grant is recorded under it, and `session`, which the
    /// caller has checked is open and the launcher's, lets the sheet offer "This session only".
    /// With a session "Add Account…" is not offered a second route: it ends the request
    /// dismissed, since the account it would add and allow gets an `Always` grant, not the
    /// session one.
    pub async fn choose_for_agent(
        &self,
        agent: &AppId,
        need: Need,
        (class, usage): (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> AccountsReply {
        self.choose(agent, need, (class, usage), window, session)
            .await
    }

    /// Removes the grants scoped to `session`, or every session grant when none is named (a
    /// starting accountd has no open session, so a session grant a file holds is one nobody can
    /// end). Audits each as `SessionGrantEnded`, saves, and returns the grants removed.
    pub async fn end_session_grants(&self, session: Option<&LauncherSession>) -> Vec<GrantId> {
        let ended: Vec<(GrantId, AppId, AccountId, LauncherSession)> = {
            let mut registry = self.lock();
            let ended = registry
                .grants
                .iter()
                .filter_map(|g| match &g.scope {
                    GrantScope::Session(own) if session.is_none_or(|only| only == own) => Some((
                        g.id.clone(),
                        g.key.app.clone(),
                        g.key.account.clone(),
                        own.clone(),
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>();
            registry
                .grants
                .retain(|g| !ended.iter().any(|(id, ..)| *id == g.id));
            ended
        };
        for (grant, app, account, session) in &ended {
            self.note(
                Some(app.clone()),
                Some(account.clone()),
                AuditEvent::SessionGrantEnded {
                    grant: grant.clone(),
                    session: session.clone(),
                },
            );
        }
        if !ended.is_empty() {
            // The grants are gone from memory whether or not the file could be written; the
            // next save writes them out.
            let _ = self.persist().await;
        }
        ended.into_iter().map(|(grant, ..)| grant).collect()
    }

    async fn choose(
        &self,
        caller: &AppId,
        need: Need,
        (class, usage): (DataClass, Usage),
        window: &ParentWindow,
        session: Option<&LauncherSession>,
    ) -> AccountsReply {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        let Some(ask) = ask_for(&self.lock(), &need, asker, session) else {
            return AccountsReply::Refused(Refusal::NoFittingAccount);
        };
        let answer = self.sheets.consent(ask, window).await;
        if answer == ConsentAnswer::AddAccount {
            return match session {
                None => self.add_and_allow_for(caller, need, asker, window).await,
                Some(_) => AccountsReply::Refused(Refusal::Dismissed),
            };
        }
        let reply = settle(
            &mut self.lock(),
            &need,
            asker,
            session,
            answer,
            self.clock.now(),
        );
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

    /// Spends `caller`'s grant when it is a `Once` one: the first use of any kind (a token
    /// issued, a relay opened) drops it, so a second use asks again.
    pub(crate) async fn spend_once(&self, caller: &AppId, grant: &GrantId) -> Result<(), Refusal> {
        let spent = {
            let mut registry = self.lock();
            let once = registry
                .grant_of(caller, grant)
                .is_some_and(|g| g.scope == GrantScope::Once);
            if once {
                registry.grants.retain(|g| g.id != *grant);
            }
            once
        };
        if spent {
            self.persist().await?;
        }
        Ok(())
    }

    /// A refresh the provider refused (`Unauthorized`) leaves the account `NeedsReauth`, which
    /// the host announces; every other error is only the app's refusal.
    pub(crate) async fn refused_refresh(
        &self,
        account: &AccountId,
        error: ProviderError,
    ) -> Refusal {
        let refusal = provider_refusal(error);
        if refusal == Refusal::NeedsReauth {
            self.set_state(account, AccountState::NeedsReauth).await;
        }
        refusal
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
                Some((account.clone(), g.scope.clone(), g.decision, g.key.kind))
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
            Err(error) => {
                return AccountsReply::Refused(self.refused_refresh(&account.id, error).await);
            }
        };
        // The grant's kind, so the token reaches that kind's service alone.
        let token = match session.access_token_for(audience, kind).await {
            Ok(token) => token,
            Err(error) => {
                return AccountsReply::Refused(self.refused_refresh(&account.id, error).await);
            }
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
        if scope == GrantScope::Once
            && let Err(refusal) = self.spend_once(caller, grant).await
        {
            return AccountsReply::Refused(refusal);
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
