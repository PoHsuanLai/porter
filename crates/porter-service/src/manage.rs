//! What Settings does to the registry (design/22 §9.4, PLAN §2.5 and §2.9): remove an account
//! with a best-effort revoke at its provider, switch a kind on or off, revoke any grant, and
//! mark an account's state. The transport (accountd's settings module) decides who may ask.

use crate::audit::AuditSink;
use crate::clock::Clock;
use crate::service::AccountService;
use crate::sheets::Sheets;
use crate::store::RegistryStore;
use crate::token::{provider_refusal, secret_purpose};
use porter_core::audit::AuditEvent;
use porter_core::store::AccountToggle;
use porter_core::wire::Refusal;
use porter_core::{
    AbsentReason, AccountId, AccountState, CapabilityKind, Claim, GrantId, Offer, Provenance,
    SecretKey, Subject, Toggle, effective,
};
use porter_provider::{Presented, Provider, ProviderSession, RevokeOutcome};
use porter_secrets::{Secrets, SecretsError};

/// What removing an account did at the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeReport {
    /// The provider was asked, and answered this.
    Asked(RevokeOutcome),
    /// The provider could not be asked (no credential, unreachable, refused): the wipe went on.
    Skipped,
}

impl<P: Provider, S: Secrets, U: Sheets, K: Clock, R: RegistryStore, A: AuditSink>
    AccountService<P, S, U, K, R, A>
{
    /// Removes an account (PLAN §2.9): asks its provider to stop honouring the credential, best
    /// effort, then wipes its secrets, grants, toggles and row, and audits `Removed`. The
    /// provider's answer never stops the wipe.
    pub async fn remove_with_revoke(&self, id: &AccountId) -> Result<RevokeReport, SecretsError> {
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

    /// Asks the provider what the account can do now and records it (the claims, with the
    /// account's toggles applied). A refresh token the provider rotated while it looked is read
    /// back from a session opened afterwards (`renewed`) and stored, so the next sign-in
    /// check does not present a token the issuer has since replaced.
    pub async fn rediscover(&self, id: &AccountId) -> Result<Vec<Claim>, Refusal> {
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
        let presented = self
            .presented(&account)
            .await
            .map_err(crate::token::secrets_refusal)?;
        let claims = provider
            .discover(&account, &presented)
            .await
            .map_err(provider_refusal)?;
        let session = provider
            .open(&account.id, presented)
            .await
            .map_err(provider_refusal)?;
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

    /// Switches one kind of one account on or off: the toggle row is kept, and the account's
    /// effective capabilities follow (a kind turned off reads `Absent { TurnedOff }`; turned on
    /// again it reads as its provider file declares it until the next discovery refreshes it).
    pub async fn set_toggle(
        &self,
        id: &AccountId,
        kind: CapabilityKind,
        toggle: Toggle,
    ) -> Result<(), Refusal> {
        {
            let mut registry = self.lock();
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
        }
        self.persist().await
    }

    /// Withdraws any grant, whoever holds it (Settings' "Revoke"). `UnknownGrant` when none has
    /// that id.
    pub async fn revoke_grant(&self, grant: &GrantId) -> Result<(), Refusal> {
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

    /// Sets an account's state (`NeedsReauth` when its credential was refused, `Ok` again once
    /// it is signed in). Whether it changed.
    pub async fn set_state(&self, id: &AccountId, state: AccountState) -> bool {
        let changed = {
            let mut registry = self.lock();
            registry
                .accounts
                .iter_mut()
                .find(|a| a.id == *id)
                .is_some_and(|a| std::mem::replace(&mut a.state, state) != state)
        };
        if changed {
            let _ = self.persist().await;
        }
        changed
    }
}
