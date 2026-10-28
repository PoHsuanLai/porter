//! What Settings does to the accounts (design/22 §9.4, PLAN §2.5 and §2.9): remove an account
//! with a best-effort revoke at its provider, look at what it can do again, and switch a kind
//! on or off. The transport (accountd's settings module) decides who may ask.

use crate::Accounts;
use porter_capability::{
    AbsentReason, CapabilityKind, Claim, Offer, Provenance, Subject, Toggle, effective,
};
use porter_core::store::AccountToggle;
use porter_core::{AccountId, SecretKey};
use porter_provider::Provider;
use porter_provider::ProviderSession;
use porter_provider::{Presented, RevokeOutcome};
use porter_secrets::Secrets;
use porter_secrets::SecretsError;
use porter_service_core::Ports;
use porter_service_core::audit::AuditEvent;
use porter_wire::Refusal;

impl<E: Ports> Accounts<'_, E> {
    /// See [`AccountsApi::remove_with_revoke`](crate::AccountsApi::remove_with_revoke):
    /// what the provider answered, or `None` when it could not be asked (no credential,
    /// unreachable, refused) and the wipe went on.
    pub(crate) async fn remove_with_revoke(
        &self,
        id: &AccountId,
    ) -> Result<Option<RevokeOutcome>, SecretsError> {
        let account = self
            .ctx
            .lock()
            .accounts
            .iter()
            .find(|a| a.id == *id)
            .cloned();
        let report = match account {
            Some(account) => self.revoke_at_provider(&account).await,
            None => None,
        };
        self.wipe_account(id).await?;
        Ok(report)
    }

    async fn revoke_at_provider(&self, account: &porter_core::Account) -> Option<RevokeOutcome> {
        let provider = self
            .ctx
            .providers()
            .iter()
            .find(|p| p.spec().id == account.provider)?;
        let presented = self.presented(account).await.ok()?;
        provider.revoke(account, &presented).await.ok()
    }

    /// What the account presents to its provider: its stored credential, or nothing for a
    /// provider that needs none.
    async fn presented(&self, account: &porter_core::Account) -> Result<Presented, SecretsError> {
        match account.auth.facts().secret {
            None => Ok(Presented::Anonymous),
            Some(purpose) => {
                let key = SecretKey {
                    account: account.id.clone(),
                    purpose,
                };
                self.ctx
                    .secrets()
                    .get(&key)
                    .await
                    .map(Presented::Credential)
            }
        }
    }

    /// See [`AccountsApi::rediscover`](crate::AccountsApi::rediscover).
    pub(crate) async fn rediscover(&self, id: &AccountId) -> Result<Vec<Claim>, Refusal> {
        let account = self
            .ctx
            .lock()
            .accounts
            .iter()
            .find(|a| a.id == *id)
            .cloned()
            .ok_or(Refusal::UnknownGrant)?;
        let provider = self
            .ctx
            .providers()
            .iter()
            .find(|p| p.spec().id == account.provider)
            .ok_or(Refusal::Unavailable)?;
        // The credential is read, used and (if it rotated) stored again under the account's
        // turn at its issuer, so a token being issued at the same time never presents one this
        // has replaced, nor writes one back over this one.
        let flight = self.ctx.flights().of(&account.id);
        let _turn = flight.gate.lock().await;
        let presented = self
            .presented(&account)
            .await
            .map_err(porter_service_core::refusal::secrets_refusal)?;
        let claims = match provider.discover(&account, &presented).await {
            Ok(claims) => claims,
            Err(error) => return Err(self.ctx.refused_refresh(&account.id, error).await),
        };
        let session = match provider.open(&account.id, presented).await {
            Ok(session) => session,
            Err(error) => return Err(self.ctx.refused_refresh(&account.id, error).await),
        };
        if let (Some(renewed), Some(purpose)) = (session.renewed(), account.auth.facts().secret) {
            let key = SecretKey {
                account: account.id.clone(),
                purpose,
            };
            self.ctx
                .secrets()
                .put(&key, &renewed)
                .await
                .map_err(porter_service_core::refusal::secrets_refusal)?;
        }
        {
            let mut registry = self.ctx.lock();
            let off: Vec<_> = registry
                .toggles
                .iter()
                .filter(|t| t.account == *id)
                .map(|t| porter_capability::KindToggle {
                    kind: t.kind,
                    toggle: t.toggle,
                })
                .collect();
            if let Some(held) = registry.accounts.iter_mut().find(|a| a.id == *id) {
                held.capabilities = effective(&claims, &off);
            }
        }
        self.ctx.persist().await?;
        Ok(claims)
    }

    /// See [`AccountsApi::set_toggle`](crate::AccountsApi::set_toggle).
    pub(crate) async fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> Result<(), Refusal> {
        let changed = {
            let mut registry = self.ctx.lock();
            // Only a kind switched off has a row.
            let was = match registry
                .toggles
                .iter()
                .any(|t| t.account == *id && t.kind == kind)
            {
                true => Toggle::Off,
                false => Toggle::On,
            };
            let spec = registry
                .accounts
                .iter()
                .find(|a| a.id == *id)
                .and_then(|a| self.ctx.catalog().get(&a.provider))
                .ok_or(Refusal::UnknownGrant)?;
            let declared: Vec<Claim> = spec
                .capabilities
                .iter()
                .map(|row| Claim {
                    subject: Subject::Account,
                    offer: Offer::Present(row.capability.clone()),
                    provenance: Provenance::Declared,
                })
                .collect();
            registry
                .toggles
                .retain(|t| !(t.account == *id && t.kind == kind));
            if toggle == Toggle::Off {
                registry.toggles.push(AccountToggle {
                    account: id.clone(),
                    kind,
                    toggle,
                });
            }
            let off: Vec<_> = registry
                .toggles
                .iter()
                .filter(|t| t.account == *id)
                .map(|t| porter_capability::KindToggle {
                    kind: t.kind,
                    toggle: t.toggle,
                })
                .collect();
            if let Some(account) = registry.accounts.iter_mut().find(|a| a.id == *id) {
                // What the account holds, except what a toggle replaced, plus what the file
                // declares for the kinds that were toggled off: the toggles then apply afresh.
                let mut claims: Vec<Claim> = account
                    .capabilities
                    .iter()
                    .filter(|c| {
                        !matches!(
                            &c.offer,
                            Offer::Absent {
                                reason: AbsentReason::TurnedOff,
                                ..
                            }
                        )
                    })
                    .cloned()
                    .collect();
                let missing: Vec<Claim> = declared
                    .into_iter()
                    .filter(|d| {
                        !claims
                            .iter()
                            .any(|c| c.subject == d.subject && c.offer.kind() == d.offer.kind())
                    })
                    .collect();
                claims.extend(missing);
                account.capabilities = effective(&claims, &off);
            }
            was != toggle
        };
        if changed {
            self.ctx.note(
                None,
                Some(id.clone()),
                AuditEvent::ServiceToggled { kind, toggle },
            );
        }
        self.ctx.persist().await
    }

    /// The wipe of `remove_account`: secrets, grants, toggles, the registry row, one audit line.
    pub(crate) async fn wipe_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        // The rows go first, in one step with no await, and come back if the secrets cannot be
        // deleted. A wipe dropped while it awaits the secrets therefore leaves no account
        // without its credentials; at worst credentials with no account, which nothing reads.
        let (accounts, grants, toggles) = {
            let mut registry = self.ctx.lock();
            let (gone, kept) = std::mem::take(&mut registry.accounts)
                .into_iter()
                .partition(|a| a.id == *id);
            registry.accounts = kept;
            let (dropped, kept) = std::mem::take(&mut registry.grants)
                .into_iter()
                .partition(|g| g.key.account == *id);
            registry.grants = kept;
            let (off, kept) = std::mem::take(&mut registry.toggles)
                .into_iter()
                .partition(|t| t.account == *id);
            registry.toggles = kept;
            (gone, dropped, off)
        };
        if let Err(why) = self.ctx.secrets().delete_account(id).await {
            let mut registry = self.ctx.lock();
            registry.accounts.extend(accounts);
            registry.grants.extend(grants);
            registry.toggles.extend(toggles);
            return Err(why);
        }
        self.ctx.note(None, Some(id.clone()), AuditEvent::Removed);
        // The in-memory registry is already without the account; a store that cannot be
        // written keeps the old row until the next save, and the host sees it on its own save.
        let _ = self.ctx.persist().await;
        Ok(())
    }
}
