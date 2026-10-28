//! What an app's grants do: ask the person for one (`choose`), end one (`revoke`, the end of a
//! launcher session, the first use of a `Once` grant), and answer which accounts and grants an
//! app can see.

use crate::audit::{AuditEvent, AuditSink};
use crate::choose::{ask_for, settle};
use crate::ctx::Ctx;
use crate::registry::Asker;
use crate::sheets::Sheets;
use porter_core::consent::{ConsentAnswer, GrantScope, Usage, Verdict, availability};
use porter_core::store::RegistryStore;
use porter_core::wire::{ParentWindow, Refusal};
use porter_core::{AccountId, GrantId};
use porter_core::{AccountsReply, AppId, DataClass, LauncherSession, Need};
use porter_ids::clock::Clock;
use porter_provider::Provider;
use porter_secrets::Secrets;

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    Ctx<P, S, U, K, R, A>
{
    /// The accounts that fit `need` for `caller`, as `Query` answers.
    pub(crate) fn query(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> AccountsReply {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        AccountsReply::Candidates(self.lock().candidates(need, asker))
    }

    /// What is available for `need`, as `Availability` answers.
    pub(crate) fn availability_of(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> AccountsReply {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        let verdicts = self.lock().verdicts(need, asker);
        AccountsReply::Availability(availability(&verdicts, self.catalog.catalog(need)))
    }

    /// The grants `caller` holds, as `ListGrants` answers.
    pub(crate) fn grants_of(&self, caller: &AppId) -> AccountsReply {
        let grants = self
            .lock()
            .grants
            .iter()
            .filter(|g| g.key.app == *caller)
            .cloned()
            .collect();
        AccountsReply::Grants(grants)
    }

    pub(crate) async fn choose(
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
        let ask = {
            let registry = self.lock();
            // Choose draws the sheet each time, except when the person has already said Don't
            // Allow for every account that fits: asking again would badger them, and undoing a
            // Don't Allow is theirs to do, in Settings. This holds for a launcher's session ask
            // too, which is held under the agent's app.
            let verdicts = registry.verdicts(&need, asker);
            if !verdicts.is_empty() && verdicts.iter().all(|v| *v == Verdict::Denied) {
                return AccountsReply::Refused(Refusal::Denied);
            }
            ask_for(&registry, &need, asker, session)
        };
        let Some(ask) = ask else {
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

    pub(crate) async fn revoke(&self, caller: &AppId, grant: &GrantId) -> AccountsReply {
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

    /// The body of [`AccountService::spend_once`](crate::AccountService::spend_once).
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

    /// The body of [`AccountService::end_session_grants`](crate::AccountService::end_session_grants).
    pub(crate) async fn end_session_grants(
        &self,
        session: Option<&LauncherSession>,
    ) -> Vec<GrantId> {
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
}
