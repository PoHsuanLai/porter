//! The permits to use the person's AI keys: what each account that serves language models says
//! for an app, the permit an app already holds, and the key behind a permit. The host that
//! sends a key to an AI company asks for it here; the key is read from the service's own
//! secrets and handed over once.

use crate::Grants;
use porter_capability::{CapabilityKind, Offer};
use porter_core::consent::{Decision, Usage};
use porter_core::{AppId, Audience, Candidate, DataClass, GrantId, LlmVerdict};
use porter_core::{GrantKey, SecretText, SpaceScope, decide_key};
use porter_match::{Match, Need, llm_need, matches};
use porter_service_accounts::AccountsApi;
use porter_service_core::Ports;
use porter_service_core::audit::AuditEvent;
use porter_service_core::registry::Asker;
use porter_wire::Refusal;

impl<E: Ports> Grants<'_, E> {
    /// See [`GrantsApi::llm_verdicts`](crate::GrantsApi::llm_verdicts).
    pub(crate) fn llm_verdicts_of(
        &self,
        app: &AppId,
        class: DataClass,
        usage: Usage,
        provider_label: &dyn Fn(&porter_core::ProviderId) -> Option<String>,
    ) -> Vec<LlmVerdict> {
        let registry = self.ctx.lock();
        let need = llm_need();
        registry
            .accounts
            .iter()
            .filter_map(|account| {
                let claim = account.capabilities.iter().find(|claim| {
                    matches!(claim.offer, Offer::Present(_))
                        && matches(&need, &claim.offer) == Match::Fits
                })?;
                let key = GrantKey {
                    app: app.clone(),
                    account: account.id.clone(),
                    kind: claim.offer.kind(),
                    class,
                    usage,
                    space: SpaceScope::Any,
                    reach: porter_capability::StorageScope::AppFolder,
                };
                Some(LlmVerdict::new(
                    account.id.clone(),
                    account.provider.as_str().to_owned(),
                    account.label.0.clone(),
                    provider_label(&account.provider),
                    account.state,
                    decide_key(&registry.grants, &key),
                ))
            })
            .collect()
    }

    /// See [`GrantsApi::release_key`](crate::GrantsApi::release_key).
    pub(crate) async fn release_key(&self, grant: &GrantId) -> Result<SecretText, Refusal> {
        let (app, account) = {
            let registry = self.ctx.lock();
            let held = registry
                .grants
                .iter()
                .find(|g| g.id == *grant && g.decision == Decision::Allow)
                .ok_or(Refusal::UnknownGrant)?;
            if held.key.kind != CapabilityKind::Llm {
                return Err(Refusal::AudienceNotGranted);
            }
            let account = registry
                .accounts
                .iter()
                .find(|a| a.id == held.key.account)
                .ok_or(Refusal::UnknownGrant)?;
            registry.llm_usable(account)?;
            (held.key.app.clone(), account.id.clone())
        };
        let key = self.accounts.api_key(&account).await?;
        self.ctx.note(
            Some(app.clone()),
            Some(account),
            AuditEvent::KeyResolved {
                grant: grant.clone(),
                audience: Audience(app.name.as_str().to_owned()),
            },
        );
        // An allow-once permit is used up by the first key read that succeeded; a failed read
        // returned before this and left it.
        self.spend_once(&app, grant).await?;
        Ok(key)
    }

    /// See [`GrantsApi::existing_permit`](crate::GrantsApi::existing_permit).
    pub(crate) fn existing_permit_of(
        &self,
        app: &AppId,
        need: &Need,
        class: DataClass,
        usage: Usage,
    ) -> Option<Candidate> {
        let asker = Asker { app, class, usage };
        let registry = self.ctx.lock();
        let given = |held: &Candidate| {
            registry
                .grants
                .iter()
                .find(|g| g.id == held.grant)
                .map(|g| g.at)
        };
        registry
            .candidates(need, asker)
            .into_iter()
            .max_by_key(given)
    }
}
