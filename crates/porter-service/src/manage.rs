//! What Settings does to the registry (design/22 §9.4, PLAN §2.5 and §2.9): remove an account
//! with a best-effort revoke at its provider, switch a kind on or off, revoke any grant, and
//! mark an account's state. The transport (accountd's settings module) decides who may ask.

use crate::audit::AuditEvent;
use crate::audit::AuditSink;
use crate::ctx::Ctx;
use crate::sheets::Sheets;
use crate::token::secret_purpose;
use porter_core::store::AccountToggle;
use porter_core::store::RegistryStore;
use porter_core::{
    AbsentReason, AppId, CapabilityKind, Claim, Offer, Provenance, SecretKey, Subject, Toggle,
    effective,
};
use porter_core::{AccountId, GrantId};
use porter_ids::clock::Clock;
use porter_provider::{Presented, Provider, ProviderSession, RevokeOutcome};
use porter_secrets::{Secrets, SecretsError};
use porter_wire::Refusal;

/// What removing an account did at the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeReport {
    /// The provider was asked, and answered this.
    Asked(RevokeOutcome),
    /// The provider could not be asked (no credential, unreachable, refused): the wipe went on.
    Skipped,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    Ctx<P, S, U, K, R, A>
{
    /// The body of [`AccountService::remove_with_revoke`](crate::AccountService::remove_with_revoke).
    pub(crate) async fn remove_with_revoke(
        &self,
        id: &AccountId,
    ) -> Result<RevokeReport, SecretsError> {
        let account = self.lock().accounts.iter().find(|a| a.id == *id).cloned();
        let report = match account {
            Some(account) => self.revoke_at_provider(&account).await,
            None => RevokeReport::Skipped,
        };
        self.wipe_account(id).await?;
        Ok(report)
    }

    async fn revoke_at_provider(&self, account: &porter_core::Account) -> RevokeReport {
        let Some(provider) = self
            .providers
            .iter()
            .find(|p| p.spec().id == account.provider)
        else {
            return RevokeReport::Skipped;
        };
        let Ok(presented) = self.presented(account).await else {
            return RevokeReport::Skipped;
        };
        match provider.revoke(account, &presented).await {
            Ok(outcome) => RevokeReport::Asked(outcome),
            Err(_) => RevokeReport::Skipped,
        }
    }

    /// What the account presents to its provider: its stored credential, or nothing for a
    /// provider that needs none.
    async fn presented(&self, account: &porter_core::Account) -> Result<Presented, SecretsError> {
        match secret_purpose(account.auth) {
            None => Ok(Presented::Anonymous),
            Some(purpose) => {
                let key = SecretKey {
                    account: account.id.clone(),
                    purpose,
                };
                self.secrets.get(&key).await.map(Presented::Credential)
            }
        }
    }

    /// The body of [`AccountService::rediscover`](crate::AccountService::rediscover).
    pub(crate) async fn rediscover(&self, id: &AccountId) -> Result<Vec<Claim>, Refusal> {
        let account = self
            .lock()
            .accounts
            .iter()
            .find(|a| a.id == *id)
            .cloned()
            .ok_or(Refusal::UnknownGrant)?;
        let provider = self
            .providers
            .iter()
            .find(|p| p.spec().id == account.provider)
            .ok_or(Refusal::Unavailable)?;
        // The credential is read, used and (if it rotated) stored again under the account's
        // turn at its issuer, so a token being issued at the same time never presents one this
        // has replaced, nor writes one back over this one.
        let flight = self.flights.of(&account.id);
        let _turn = flight.gate.lock().await;
        let presented = self
            .presented(&account)
            .await
            .map_err(crate::token::secrets_refusal)?;
        let claims = match provider.discover(&account, &presented).await {
            Ok(claims) => claims,
            Err(error) => return Err(self.refused_refresh(&account.id, error).await),
        };
        let session = match provider.open(&account.id, presented).await {
            Ok(session) => session,
            Err(error) => return Err(self.refused_refresh(&account.id, error).await),
        };
        if let (Some(renewed), Some(purpose)) = (session.renewed(), secret_purpose(account.auth)) {
            let key = SecretKey {
                account: account.id.clone(),
                purpose,
            };
            self.secrets
                .put(&key, &renewed)
                .await
                .map_err(crate::token::secrets_refusal)?;
        }
        {
            let mut registry = self.lock();
            let off: Vec<_> = registry
                .toggles
                .iter()
                .filter(|t| t.account == *id)
                .map(|t| porter_core::KindToggle {
                    kind: t.kind,
                    toggle: t.toggle,
                })
                .collect();
            if let Some(held) = registry.accounts.iter_mut().find(|a| a.id == *id) {
                held.capabilities = effective(&claims, &off);
            }
        }
        self.persist().await?;
        Ok(claims)
    }

    /// The body of [`AccountService::set_toggle`](crate::AccountService::set_toggle).
    pub(crate) async fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> Result<(), Refusal> {
        let changed = {
            let mut registry = self.lock();
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
                .and_then(|a| self.catalog.get(&a.provider))
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
                .map(|t| porter_core::KindToggle {
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
            self.note(
                None,
                Some(id.clone()),
                AuditEvent::ServiceToggled { kind, toggle },
            );
        }
        self.persist().await
    }

    /// The body of [`AccountService::audit_settings`](crate::AccountService::audit_settings).
    pub(crate) fn audit_settings(&self, event: AuditEvent) {
        self.note(None, None, event);
    }

    /// The body of [`AccountService::audit_app`](crate::AccountService::audit_app).
    pub(crate) fn audit_app(&self, app: AppId, event: AuditEvent) {
        self.note(Some(app), None, event);
    }

    /// The body of [`AccountService::revoke_grant`](crate::AccountService::revoke_grant).
    pub(crate) async fn revoke_grant(&self, grant: &GrantId) -> Result<(), Refusal> {
        let held = {
            let mut registry = self.lock();
            let held = registry.grants.iter().find(|g| g.id == *grant).cloned();
            registry.grants.retain(|g| g.id != *grant);
            held
        }
        .ok_or(Refusal::UnknownGrant)?;
        self.note(
            Some(held.key.app),
            Some(held.key.account),
            AuditEvent::Revoked {
                grant: grant.clone(),
            },
        );
        self.persist().await
    }

    /// The wipe of `remove_account`: secrets, grants, toggles, the registry row, one audit line.
    pub(crate) async fn wipe_account(&self, id: &AccountId) -> Result<(), SecretsError> {
        // The rows go first, in one step with no await, and come back if the secrets cannot be
        // deleted. A wipe dropped while it awaits the secrets therefore leaves no account
        // without its credentials; at worst credentials with no account, which nothing reads.
        let (accounts, grants, toggles) = {
            let mut registry = self.lock();
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
        if let Err(why) = self.secrets.delete_account(id).await {
            let mut registry = self.lock();
            registry.accounts.extend(accounts);
            registry.grants.extend(grants);
            registry.toggles.extend(toggles);
            return Err(why);
        }
        self.note(None, Some(id.clone()), AuditEvent::Removed);
        // The in-memory registry is already without the account; a store that cannot be
        // written keeps the old row until the next save, and the host sees it on its own save.
        let _ = self.persist().await;
        Ok(())
    }
}
