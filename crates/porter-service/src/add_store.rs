//! What a finished sign-in becomes: the account row, its secrets, its toggles, its audit line
//! and, for "Add, and allow", its first grant. Each store is all or nothing: a secret that
//! cannot be filed or a registry that cannot be written leaves nothing behind.

use crate::add_flow::AllowFor;
use crate::audit::AuditSink;
use crate::choose::settle;
use crate::clock::Clock;
use crate::registry::{Asker, Registry};
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use porter_core::audit::AuditEvent;
use porter_core::consent::{ConsentAnswer, GrantScope};
use porter_core::sheet::ServiceChoice;
use porter_core::store::AccountToggle;
use porter_core::{
    Account, AccountId, AccountState, AccountsReply, AppId, AuthKind, KindToggle, LoginName,
    ProviderId, SecretKey, Toggle, effective,
};
use porter_provider::{Provider, ProviderSpec, Signed};
use porter_secrets::Secrets;
use std::collections::HashSet;

/// An account id for `label` at `provider` that no account in `registry` has: the provider's id
/// and the label as a slug, with a number after it when that is taken.
pub(crate) fn fresh_id(registry: &Registry, provider: &ProviderId, label: &str) -> AccountId {
    let slug: String = label
        .to_ascii_lowercase()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '.' | '_' => c,
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let taken = |id: &AccountId| registry.accounts.iter().any(|a| a.id == *id);
    let stem = match slug.is_empty() {
        true => provider.to_string(),
        false => format!("{provider}-{slug}"),
    };
    let suffixes = std::iter::once(String::new()).chain((2..).map(|n| format!("-{n}")));
    suffixes
        .filter_map(|suffix| {
            // An id is at most 64 bytes, and the number after a cut stem must stay whole.
            let room = 64usize.saturating_sub(suffix.len());
            let cut: String = stem.chars().take(room).collect();
            AccountId::parse(&format!(
                "{}{suffix}",
                cut.trim_end_matches(['-', '.', '_'])
            ))
            .ok()
        })
        .find(|id| !taken(id))
        .unwrap_or_else(|| unreachable!("the suffixes are unbounded and each id is well formed"))
}

/// The state a new account starts in: working, except an agent that signs itself in, which is
/// waiting for the agent to say it is (`Peer.SetAgentState`); porter has no way to know.
fn first_state(auth: AuthKind) -> AccountState {
    match auth {
        AuthKind::AgentLogin => AccountState::NeedsLogin,
        _ => AccountState::Ok,
    }
}

/// The logins an account's endpoints are for.
fn logins(endpoints: &[porter_core::ServiceEndpoint]) -> HashSet<&LoginName> {
    endpoints.iter().map(|e| &e.login).collect()
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Stores the account `signed` describes, with the person's service `choices`, and the
    /// grant for `allow` when the new account meets its need.
    pub(crate) async fn store_new(
        &self,
        caller: &AppId,
        spec: &ProviderSpec,
        signed: &Signed,
        choices: &[ServiceChoice],
        allow: Option<&AllowFor>,
    ) -> Result<AccountId, ()> {
        if signed.endpoints.iter().any(|e| e.check().is_err()) {
            return Err(());
        }
        let off: Vec<KindToggle> = choices
            .iter()
            .filter(|c| c.toggle == Toggle::Off)
            .map(|c| KindToggle {
                kind: c.kind,
                toggle: Toggle::Off,
            })
            .collect();
        let account = {
            let mut registry = self.lock();
            let id = fresh_id(&registry, &spec.id, &signed.label.0);
            let account = Account {
                id,
                provider: spec.id.clone(),
                label: signed.label.clone(),
                state: first_state(spec.auth.kind),
                auth: spec.auth.kind,
                capabilities: effective(&signed.claims, &off),
                restriction: signed.restriction.clone(),
                endpoints: signed.endpoints.clone(),
            };
            registry.accounts.push(account.clone());
            registry.toggles.extend(off.iter().map(|t| AccountToggle {
                account: account.id.clone(),
                kind: t.kind,
                toggle: t.toggle,
            }));
            account
        };
        for (purpose, credential) in &signed.credentials {
            let key = SecretKey {
                account: account.id.clone(),
                purpose: *purpose,
            };
            if self.secrets.put(&key, credential).await.is_err() {
                self.undo_add(&account.id).await;
                return Err(());
            }
        }
        let granted = allow.and_then(|ask| self.grant_first(caller, &account.id, ask));
        if self.persist().await.is_err() {
            self.undo_add(&account.id).await;
            return Err(());
        }
        self.note(
            Some(caller.clone()),
            Some(account.id.clone()),
            AuditEvent::SignedIn,
        );
        if let Some(AccountsReply::Chosen(candidate)) = granted {
            self.note(
                Some(caller.clone()),
                Some(account.id.clone()),
                AuditEvent::Granted {
                    grant: candidate.grant,
                    kind: ask_kind(allow),
                },
            );
        }
        Ok(account.id)
    }

    /// The first grant of "Add, and allow": always, for the app that asked, when the new account
    /// meets what it asked for.
    fn grant_first(
        &self,
        caller: &AppId,
        account: &AccountId,
        ask: &AllowFor,
    ) -> Option<AccountsReply> {
        let asker = Asker {
            app: caller,
            class: ask.class,
            usage: ask.usage,
        };
        let answer = ConsentAnswer::Allow {
            account: account.clone(),
            scope: GrantScope::Always,
        };
        let reply = settle(
            &mut self.lock(),
            &ask.need,
            asker,
            None,
            answer,
            self.clock.now(),
        );
        matches!(reply, AccountsReply::Chosen(_)).then_some(reply)
    }

    /// Takes back an add that could not be finished: the row, its toggles and grants, and
    /// whatever secrets were filed.
    async fn undo_add(&self, id: &AccountId) {
        {
            let mut registry = self.lock();
            registry.accounts.retain(|a| a.id != *id);
            registry.grants.retain(|g| g.key.account != *id);
            registry.toggles.retain(|t| t.account != *id);
        }
        // Nothing more can be done for a secret that cannot be deleted.
        let _ = self.secrets.delete_account(id).await;
    }

    /// Files the credentials of a sign-in again under the account's id, sets it working, and
    /// audits it. A different person's login is refused: the account keeps its owner.
    pub(crate) async fn store_renewed(
        &self,
        caller: &AppId,
        account: &AccountId,
        signed: &Signed,
    ) -> Result<(), ()> {
        let held = self
            .lock()
            .accounts
            .iter()
            .find(|a| a.id == *account)
            .map(|a| a.endpoints.clone())
            .ok_or(())?;
        if !held.is_empty()
            && !signed.endpoints.is_empty()
            && logins(&held) != logins(&signed.endpoints)
        {
            return Err(());
        }
        for (purpose, credential) in &signed.credentials {
            let key = SecretKey {
                account: account.clone(),
                purpose: *purpose,
            };
            self.secrets.put(&key, credential).await.map_err(|_| ())?;
        }
        let was = {
            let mut registry = self.lock();
            let row = registry
                .accounts
                .iter_mut()
                .find(|a| a.id == *account)
                .ok_or(())?;
            // Signing in again makes a refused credential good. An agent account has none: the
            // agent says when it is signed in, so the state stays what the agent last said.
            let next = match row.auth {
                AuthKind::AgentLogin => row.state,
                _ => AccountState::Ok,
            };
            // The date of this sign-in starts the next seven days.
            if signed.restriction.signed_in.is_some() {
                row.restriction.signed_in = signed.restriction.signed_in;
            }
            std::mem::replace(&mut row.state, next)
        };
        if self.persist().await.is_err() {
            if let Some(row) = self.lock().accounts.iter_mut().find(|a| a.id == *account) {
                row.state = was;
            }
            return Err(());
        }
        self.note(
            Some(caller.clone()),
            Some(account.clone()),
            AuditEvent::Reauthed,
        );
        Ok(())
    }
}

fn ask_kind(allow: Option<&AllowFor>) -> porter_core::CapabilityKind {
    allow.map_or_else(
        || unreachable!("a grant was made only for an ask"),
        |ask| ask.need.kind(),
    )
}

#[cfg(test)]
mod tests;
