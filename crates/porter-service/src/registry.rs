//! The registry's state and the pure answers over it: which accounts fit a need, what the
//! consent store says for each, and the candidates a caller may see.

use porter_core::consent::{Grant, GrantKey, Usage, Verdict, decide};
use porter_core::{
    Account, AppId, Candidate, Capability, Claim, DataClass, GrantId, Match, Need, Offer, matches,
};

/// Every account and every grant accountd holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Registry {
    /// The accounts.
    pub accounts: Vec<Account>,
    /// The consent store.
    pub grants: Vec<Grant>,
}

/// One account whose effective capability meets a need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fit<'a> {
    pub(crate) account: &'a Account,
    pub(crate) claim: &'a Claim,
    pub(crate) capability: &'a Capability,
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
        }
    }
}

impl Registry {
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
            .map(|fit| decide(&self.grants, &asker.key(fit)))
            .collect()
    }

    /// The fitting accounts `asker` holds a grant for, as candidates.
    pub(crate) fn candidates(&self, need: &Need, asker: Asker<'_>) -> Vec<Candidate> {
        self.fitting(need)
            .iter()
            .filter_map(|fit| match decide(&self.grants, &asker.key(fit)) {
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
}

/// The candidate for one fitting account under `grant`.
pub(crate) fn candidate(fit: &Fit<'_>, grant: GrantId) -> Candidate {
    Candidate {
        account: fit.account.id.clone(),
        label: fit.account.label.clone(),
        provider: fit.account.provider.clone(),
        subject: fit.claim.subject.clone(),
        capability: fit.capability.clone(),
        restriction: fit.account.restriction.clone(),
        grant,
    }
}
