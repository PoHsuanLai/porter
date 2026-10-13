//! The AI accounts over inferd's own hosted service: the verdicts are read from its registry and
//! permits, and a key is read from its own secret store in this process (no descriptor, no bus).
//! [`LocalKeys`] is what `Cloud` asks; the same object takes the local-runtime reports of the
//! probe ([`Reports`](crate::report::Reports)).

use super::desk::{KeyDesk, usable};
use super::host::KeyHost;
use crate::cloud::accountd::{
    AccountVerdict, Accountd, AccountdFault, Boxed, llm_need,
};
use crate::probe::Runtime;
use crate::probed::Standing;
use crate::report::{ReportFault, Reports};
use porter_core::consent::{Decision, GrantKey, Usage, decide_key};
use porter_core::wire::Refusal;
use porter_core::{
    AccountId, AccountState, AppId, CapabilityKind, Claim, DataClass, GrantId, Match, Offer,
    ProviderId, SecretText, SpaceScope, matches,
};
use porter_families::set::ProviderNames;
use porter_service::LocalFault;
use std::sync::Arc;

/// The AI accounts of the hosted service `H`, with the desk that reads their keys.
pub struct LocalKeys<H> {
    host: Arc<H>,
    desk: Arc<dyn KeyDesk>,
    names: ProviderNames,
}

impl<H> std::fmt::Debug for LocalKeys<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalKeys").finish_non_exhaustive()
    }
}

impl<H> Clone for LocalKeys<H> {
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            desk: Arc::clone(&self.desk),
            names: self.names.clone(),
        }
    }
}

impl<H: KeyHost> LocalKeys<H> {
    /// The accounts of `host`, keys read through `desk`, providers named by `names`.
    pub fn new(host: Arc<H>, desk: Arc<dyn KeyDesk>, names: ProviderNames) -> Self {
        Self { host, desk, names }
    }
}

/// The word a state is told as.
pub(crate) fn state_slug(state: AccountState) -> &'static str {
    match state {
        AccountState::Ok => "ok",
        AccountState::NeedsReauth => "needs_reauth",
        AccountState::Offline => "offline",
        AccountState::Limited => "limited",
        AccountState::NeedsLogin => "needs_login",
    }
}

impl<H: KeyHost> Accountd for LocalKeys<H> {
    fn verdicts<'a>(
        &'a self,
        app: &'a AppId,
        class: DataClass,
        usage: Usage,
    ) -> Boxed<'a, Result<Vec<AccountVerdict>, AccountdFault>> {
        Box::pin(async move {
            let registry = self.host.registry();
            let need = llm_need();
            Ok(registry
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
                    };
                    Some(AccountVerdict {
                        account: account.id.clone(),
                        provider: Some(account.provider.as_str().to_owned()),
                        label: Some(account.label.0.clone()),
                        provider_label: self
                            .names
                            .label_of(&account.provider)
                            .map(str::to_owned),
                        state: Some(account.state),
                        verdict: decide_key(&registry.grants, &key),
                    })
                })
                .collect())
        })
    }

    fn key<'a>(&'a self, grant: &'a GrantId) -> Boxed<'a, Result<SecretText, AccountdFault>> {
        Box::pin(async move {
            let registry = self.host.registry();
            let held = registry
                .grants
                .iter()
                .find(|g| g.id == *grant && g.decision == Decision::Allow)
                .ok_or(Refusal::UnknownGrant)?;
            if held.key.kind != CapabilityKind::Llm {
                return Err(Refusal::AudienceNotGranted.into());
            }
            let account = registry
                .accounts
                .iter()
                .find(|a| a.id == held.key.account)
                .ok_or(Refusal::UnknownGrant)?;
            usable(&registry, account)?;
            let key = self.desk.read(&account.id).await?;
            self.desk.note(&held.key.app, &account.id, grant);
            Ok(key)
        })
    }
}

impl<H: KeyHost> Reports for LocalKeys<H> {
    fn report<'a>(
        &'a self,
        runtime: Runtime,
        claims: &'a [Claim],
        standing: Standing,
    ) -> Boxed<'a, Result<AccountId, ReportFault>> {
        Box::pin(async move {
            let provider = ProviderId::parse(runtime.provider())
                .map_err(|_| ReportFault::Rejected)?;
            let state = match standing {
                Standing::Online => AccountState::Ok,
                Standing::Offline => AccountState::Offline,
            };
            self.host
                .report_local(&provider, claims.to_vec(), state)
                .await
                .map_err(|fault| match fault {
                    LocalFault::Unavailable => ReportFault::Unreachable,
                    _ => ReportFault::Rejected,
                })
        })
    }
}
