//! The registry's state and the pure answers over it: which accounts fit a need, what the
//! consent store says for each, and the candidates a caller may see.

use porter_capability::{Capability, CapabilityKind, Claim, Offer};
use porter_capability::{StorageScope, VocabVersion};
use porter_core::GrantId;
use porter_core::SpaceScope;
use porter_core::store::{AccountToggle, Persisted};
use porter_core::{Account, AccountId, AppId, Candidate, DataClass, EndpointUrl, ServiceEndpoint};
use porter_core::{
    GrantKey,
    consent::{Decision, Grant, Usage, Verdict},
    decide_key,
};
use porter_match::{Match, Need, matches};
use porter_wire::Refusal;

/// Every account, every grant and every toggle accountd holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Registry {
    /// The accounts.
    pub accounts: Vec<Account>,
    /// The consent store.
    pub grants: Vec<Grant>,
    /// What the user turned off, per account and kind.
    pub toggles: Vec<AccountToggle>,
}

impl Registry {
    /// What a store holds, as the registry the service runs on.
    pub fn from_persisted(stored: Persisted) -> Self {
        Self {
            accounts: stored.accounts,
            grants: stored.grants,
            toggles: stored.toggles,
        }
    }

    /// The registry as the document a store keeps.
    pub fn persisted(&self) -> Persisted {
        Persisted {
            vocab: VocabVersion::CURRENT,
            accounts: self.accounts.clone(),
            grants: self.grants.clone(),
            toggles: self.toggles.clone(),
        }
    }
}

/// One account whose effective capability meets a need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fit<'a> {
    pub(crate) account: &'a Account,
    pub(crate) claim: &'a Claim,
    pub(crate) capability: &'a Capability,
    /// How far a grant made for the need reaches (`Need::reach`).
    pub(crate) reach: StorageScope,
}

/// Who asks, for what data, for what use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Asker<'a> {
    pub(crate) app: &'a AppId,
    pub(crate) class: DataClass,
    pub(crate) usage: Usage,
}

impl Asker<'_> {
    pub(crate) fn key(&self, fit: &Fit<'_>) -> GrantKey {
        GrantKey {
            app: self.app.clone(),
            account: fit.account.id.clone(),
            kind: fit.capability.kind(),
            class: self.class,
            usage: self.usage,
            space: SpaceScope::Any,
            reach: fit.reach,
        }
    }
}

impl Registry {
    /// Whether `app` holds a grant that allows it `account`.
    pub(crate) fn holds(&self, app: &AppId, account: &AccountId) -> bool {
        self.grants.iter().any(|g| {
            g.key.app == *app && g.key.account == *account && g.decision == Decision::Allow
        })
    }

    /// Each account's first effective capability that meets `need`, in registry order.
    pub(crate) fn fitting(&self, need: &Need) -> Vec<Fit<'_>> {
        self.accounts
            .iter()
            .filter_map(|account| {
                account
                    .capabilities
                    .iter()
                    .find_map(|claim| match &claim.offer {
                        Offer::Present(capability)
                            if matches(need, &claim.offer) == Match::Fits =>
                        {
                            Some(Fit {
                                account,
                                claim,
                                capability,
                                reach: need.reach(),
                            })
                        }
                        _ => None,
                    })
            })
            .collect()
    }

    /// The consent store's verdict on each fitting account for `asker`.
    pub(crate) fn verdicts(&self, need: &Need, asker: Asker<'_>) -> Vec<Verdict> {
        self.fitting(need)
            .iter()
            .map(|fit| decide_key(&self.grants, &asker.key(fit)))
            .collect()
    }

    /// The fitting accounts `asker` holds a grant for, as candidates.
    pub(crate) fn candidates(&self, need: &Need, asker: Asker<'_>) -> Vec<Candidate> {
        self.fitting(need)
            .iter()
            .filter_map(|fit| match decide_key(&self.grants, &asker.key(fit)) {
                Verdict::Granted { grant, .. } => Some(candidate(fit, grant)),
                Verdict::Denied | Verdict::Ask => None,
            })
            .collect()
    }

    /// The grant `id`, if it is `app`'s.
    pub(crate) fn grant_of(&self, app: &AppId, id: &GrantId) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| g.id == *id && g.key.app == *app)
    }

    /// The account and endpoint a relay opened under `app`'s grant `id` may dial, and the
    /// kind of the grant. The endpoint must be one the account holds for the grant's kind,
    /// exactly as a candidate listed it: an app never names an address of its own.
    pub(crate) fn relay_target(
        &self,
        app: &AppId,
        id: &GrantId,
        endpoint: &EndpointUrl,
    ) -> Result<RelayTarget<'_>, Refusal> {
        let (account, kind, reach) = self.grant_account(app, id)?;
        let endpoint = account
            .endpoints
            .iter()
            .filter(|e| serves(e, kind))
            .find(|e| e.url == *endpoint)
            .ok_or(Refusal::EndpointNotGranted)?;
        Ok(RelayTarget {
            account,
            endpoint,
            kind,
            reach,
        })
    }

    /// The account, kind and reach of `app`'s grant `id`, when the grant is an allowing one.
    pub(crate) fn grant_account(
        &self,
        app: &AppId,
        id: &GrantId,
    ) -> Result<(&Account, CapabilityKind, StorageScope), Refusal> {
        let grant = self
            .grant_of(app, id)
            .filter(|g| g.decision == Decision::Allow)
            .ok_or(Refusal::UnknownGrant)?;
        // A grant over another app's own Space is never used, whatever the store holds.
        if !grant.key.space_is_open() {
            return Err(Refusal::Denied);
        }
        let account = self
            .accounts
            .iter()
            .find(|a| a.id == grant.key.account)
            .ok_or(Refusal::UnknownGrant)?;
        Ok((account, grant.key.kind, grant.key.reach))
    }
}

/// What a relay is allowed to dial under one grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelayTarget<'a> {
    pub(crate) account: &'a Account,
    pub(crate) endpoint: &'a ServiceEndpoint,
    pub(crate) kind: CapabilityKind,
    pub(crate) reach: StorageScope,
}

/// Whether `endpoint` is one a grant for `kind` may reach, and one a relay can carry.
pub(crate) fn serves(endpoint: &ServiceEndpoint, kind: CapabilityKind) -> bool {
    endpoint.protocol().is_some() && endpoint.family.serves(kind)
}

/// The candidate for one fitting account under `grant`.
pub(crate) fn candidate(fit: &Fit<'_>, grant: GrantId) -> Candidate {
    let kind = fit.capability.kind();
    Candidate::new(
        fit.account.id.clone(),
        fit.account.label.clone(),
        fit.account.provider.clone(),
        fit.claim.subject.clone(),
        fit.capability.clone(),
        fit.account.restriction.clone(),
        grant,
    )
    .with_endpoints(
        fit.account
            .endpoints
            .iter()
            .filter(|e| e.family.serves(kind))
            .cloned()
            .collect(),
    )
}
