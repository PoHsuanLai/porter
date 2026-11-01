//! What an app's grants do: ask the person for one (`choose`), end one (`revoke`, the end of a
//! session, the first use of a `Once` grant), and answer which accounts and grants an app can
//! see.

use crate::Grants;
use porter_core::consent::{
    Availability, ConsentAnswer, Grant, GrantScope, Usage, Verdict, availability,
};
use porter_core::{AccountId, GrantId};
use porter_core::{AppId, Candidate, DataClass, ProviderHint, SessionToken};
use porter_ids::clock::Clock;
use porter_match::Need;
use porter_service_accounts::{AccountsApi, AllowFor};
use porter_service_core::Ports;
use porter_service_core::audit::AuditEvent;
use porter_service_core::consent::{ask_for, settle};
use porter_service_core::registry::Asker;
use porter_sheet::host::Sheets;
use porter_wire::AccountsReply;
use porter_wire::{ParentWindow, Refusal};

impl<E: Ports> Grants<'_, E> {
    /// The accounts that fit `need` for `caller` and that `caller` holds a grant for.
    pub(crate) fn candidates_of(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Vec<Candidate> {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        self.ctx.lock().candidates(need, asker)
    }

    /// What is available for `need`: whether an account fits and whether `caller` may use it.
    pub(crate) fn availability_of(
        &self,
        caller: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Availability {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        let verdicts = self.ctx.lock().verdicts(need, asker);
        availability(&verdicts, self.ctx.catalog().catalog(need))
    }

    /// The grants `caller` holds.
    pub(crate) fn grants_of(&self, caller: &AppId) -> Vec<Grant> {
        self.ctx
            .lock()
            .grants
            .iter()
            .filter(|g| g.key.app == *caller)
            .cloned()
            .collect()
    }

    pub(crate) async fn choose(
        &self,
        caller: &AppId,
        need: Need,
        (class, usage): (DataClass, Usage),
        only: Option<&AccountId>,
        window: &ParentWindow,
        session: Option<&SessionToken>,
    ) -> AccountsReply {
        let asker = Asker {
            app: caller,
            class,
            usage,
        };
        let ask = {
            let registry = self.ctx.lock();
            // Choose draws the sheet each time, except when the person has already said Don't
            // Allow for every account that fits (the one account `only` names, when it is set):
            // asking again would badger them, and undoing a Don't Allow is theirs to do, in
            // Settings. This holds for a launcher's session ask too, which is held under the
            // agent's app.
            let verdicts = registry.verdicts_of(&need, asker, only);
            if !verdicts.is_empty() && verdicts.iter().all(|v| *v == Verdict::Denied) {
                return AccountsReply::Refused(Refusal::Denied);
            }
            ask_for(&registry, &need, asker, only, session)
        };
        let Some(ask) = ask else {
            return AccountsReply::Refused(Refusal::NoFittingAccount);
        };
        let answer = self.ctx.sheets().consent(ask, window).await;
        if answer == ConsentAnswer::AddAccount {
            // An ask about one account has no "Add Account…": what it would add is another one.
            return match (session, only) {
                (None, None) => self.add_and_allow_for(caller, need, asker, window).await,
                _ => AccountsReply::Refused(Refusal::Dismissed),
            };
        }
        let reply = settle(
            &mut self.ctx.lock(),
            &need,
            asker,
            only,
            session,
            answer,
            self.ctx.clock().now(),
        );
        match &reply {
            AccountsReply::Chosen(candidate) => self.ctx.note(
                Some(caller.clone()),
                Some(candidate.account.clone()),
                AuditEvent::Granted {
                    grant: candidate.grant.clone(),
                    kind: need.kind(),
                },
            ),
            AccountsReply::Refused(Refusal::Denied) => self.ctx.note(
                Some(caller.clone()),
                None,
                AuditEvent::Denied { kind: need.kind() },
            ),
            _ => return reply,
        }
        match self.ctx.persist().await {
            Ok(()) => reply,
            Err(refusal) => AccountsReply::Refused(refusal),
        }
    }

    /// See [`GrantsApi::revoke`](crate::GrantsApi::revoke).
    pub(crate) async fn revoke(&self, caller: &AppId, grant: &GrantId) -> Result<(), Refusal> {
        let account = {
            let mut registry = self.ctx.lock();
            let Some(held) = registry.grant_of(caller, grant) else {
                return Err(Refusal::UnknownGrant);
            };
            let account = held.key.account.clone();
            registry.grants.retain(|g| g.id != *grant);
            account
        };
        self.ctx.note(
            Some(caller.clone()),
            Some(account),
            AuditEvent::Revoked {
                grant: grant.clone(),
            },
        );
        self.ctx.persist().await
    }

    /// See [`GrantsApi::revoke_grant`](crate::GrantsApi::revoke_grant).
    pub(crate) async fn revoke_grant(&self, grant: &GrantId) -> Result<(), Refusal> {
        let held = {
            let mut registry = self.ctx.lock();
            let held = registry.grants.iter().find(|g| g.id == *grant).cloned();
            registry.grants.retain(|g| g.id != *grant);
            held
        }
        .ok_or(Refusal::UnknownGrant)?;
        self.ctx.note(
            Some(held.key.app),
            Some(held.key.account),
            AuditEvent::Revoked {
                grant: grant.clone(),
            },
        );
        self.ctx.persist().await
    }

    /// The answer "Add Account…" to a consent alert: runs the add sheet with the app's ask
    /// attached, so the account added is granted to it in the same step (one grant, no second
    /// prompt, audited by the add). A cancelled or failed add stores no grant and answers as the
    /// add did; an account added that does not meet the need is kept, ungranted, and the app is
    /// told no account fits.
    pub(crate) async fn add_and_allow_for(
        &self,
        caller: &AppId,
        need: Need,
        asker: Asker<'_>,
        window: &ParentWindow,
    ) -> AccountsReply {
        let ask = AllowFor {
            need: need.clone(),
            class: asker.class,
            usage: asker.usage,
        };
        match self
            .accounts
            .add_and_allow(caller, ProviderHint::Any, window.clone(), ask)
            .await
        {
            AccountsReply::Added(account) => self
                .ctx
                .lock()
                .candidates(&need, asker)
                .into_iter()
                .find(|c| c.account == account)
                .map_or(AccountsReply::Refused(Refusal::NoFittingAccount), |c| {
                    AccountsReply::Chosen(c)
                }),
            // Not an answer to a chooser: the app asks again and is offered the account.
            AccountsReply::AlreadyAdded(_) => AccountsReply::Refused(Refusal::Unavailable),
            refused => refused,
        }
    }

    /// See [`GrantsApi::spend_once`](crate::GrantsApi::spend_once).
    pub(crate) async fn spend_once(&self, caller: &AppId, grant: &GrantId) -> Result<(), Refusal> {
        let spent = {
            let mut registry = self.ctx.lock();
            let once = registry
                .grant_of(caller, grant)
                .is_some_and(|g| g.scope == GrantScope::Once);
            if once {
                registry.grants.retain(|g| g.id != *grant);
            }
            once
        };
        if spent {
            self.ctx.persist().await?;
        }
        Ok(())
    }

    /// See [`GrantsApi::end_session_grants`](crate::GrantsApi::end_session_grants).
    pub(crate) async fn end_session_grants(&self, session: Option<&SessionToken>) -> Vec<GrantId> {
        let ended: Vec<(GrantId, AppId, AccountId, SessionToken)> = {
            let mut registry = self.ctx.lock();
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
            self.ctx.note(
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
            let _ = self.ctx.persist().await;
        }
        ended.into_iter().map(|(grant, ..)| grant).collect()
    }
}
