//! The hosted models an app may be served: stoker's curated remote entries, each through the one
//! reach (provider, model id there, price) that an account of the app's allows.
//!
//! A remote entry is a model; its reaches are the accounts that can run it. Which reach is used
//! is stoker's `reachable` (a company's own account before a gateway's; only a wire this build
//! speaks), asked with the providers the app holds a grant on. An entry no granted account
//! reaches is still a card when the app has an account on it that says `ask` or `denied`, so a
//! refusal can say "needs a grant" rather than "nothing is available" (routing's `admit` keeps
//! only granted cards, so such a card is never served).

use super::accountd::AccountVerdict;
use model_catalog::{Locality, ModelEntry, ProviderId, Reach, Wire, reachable};
use porter_core::consent::Verdict;
use porter_core::{AccountId, Billing, GrantId, Locality as Where, MicroUsd, ModelId, PriceTable};
use porter_infer::{ModelCard, ModelRef};
use std::collections::BTreeSet;

/// The wires this build speaks to a hosted model. Anthropic's Messages API waits for a stoker
/// adapter; Claude goes through OpenRouter until then.
pub const WIRES: [Wire; 1] = [Wire::OpenAiCompat];

/// A hosted model as routing sees it, and the reach behind it.
#[derive(Debug, Clone)]
pub struct RemoteModel {
    /// What routing knows: the account of the reach, the entry's id, the cloud, the reach's price.
    pub card: ModelCard,
    /// The entry.
    pub entry: ModelEntry,
    /// The reach used: the provider, the model's id there, the price, the wire.
    pub reach: Reach,
    /// What the app's grant on the account says.
    pub verdict: Verdict,
}

impl RemoteModel {
    /// The key of the model in routing and the picker.
    pub fn model_ref(&self) -> ModelRef {
        ModelRef {
            account: self.card.account.clone(),
            model: self.card.model.clone(),
        }
    }

    /// The grant that lets the app use the account, when it holds one.
    pub fn grant(&self) -> Option<&GrantId> {
        match &self.verdict {
            Verdict::Granted { grant, .. } => Some(grant),
            Verdict::Denied | Verdict::Ask => None,
        }
    }

    /// What a token costs on this reach.
    pub fn price(&self) -> PriceTable {
        price_of(&self.reach)
    }
}

/// A reach's price in porter's units.
pub fn price_of(reach: &Reach) -> PriceTable {
    PriceTable {
        input_per_mtok: MicroUsd(reach.price.input_per_mtok.0),
        output_per_mtok: MicroUsd(reach.price.output_per_mtok.0),
    }
}

/// The entries that are hosted models.
pub fn remote_entries(entries: &[ModelEntry]) -> Vec<ModelEntry> {
    entries
        .iter()
        .filter(|entry| matches!(entry.locality, Locality::Remote { .. }))
        .cloned()
        .collect()
}

/// Every provider a remote entry names a reach through.
pub fn known_providers(entries: &[ModelEntry]) -> BTreeSet<ProviderId> {
    entries
        .iter()
        .filter_map(|entry| match &entry.locality {
            Locality::Remote { reach } => Some(reach),
            Locality::OnDevice => None,
        })
        .flatten()
        .map(|reach| reach.provider.clone())
        .collect()
}

/// The provider an account is of: what accountd says, else the longest known provider id that is
/// the account id or its stem (`anthropic-2` is `anthropic`; `google-ai` is not `google`).
pub fn provider_of(account: &AccountVerdict, known: &BTreeSet<ProviderId>) -> Option<ProviderId> {
    if let Some(said) = &account.provider {
        return Some(ProviderId(said.clone()));
    }
    let id = account.account.as_str();
    known
        .iter()
        .filter(|provider| {
            id == provider.0
                || id
                    .strip_prefix(provider.0.as_str())
                    .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|provider| provider.0.len())
        .cloned()
}

/// Which verdicts a pass over the accounts takes.
type Wanted = fn(&Verdict) -> bool;

/// The first account of `provider` with this kind of verdict.
fn account_with<'a>(
    accounts: &'a [(AccountVerdict, ProviderId)],
    provider: &ProviderId,
    wanted: Wanted,
) -> Option<&'a AccountVerdict> {
    accounts
        .iter()
        .find(|(one, of)| of == provider && wanted(&one.verdict))
        .map(|(one, _)| one)
}

fn is_granted(verdict: &Verdict) -> bool {
    matches!(verdict, Verdict::Granted { .. })
}

fn is_ask(verdict: &Verdict) -> bool {
    matches!(verdict, Verdict::Ask)
}

fn is_denied(verdict: &Verdict) -> bool {
    matches!(verdict, Verdict::Denied)
}

fn card_of(entry: &ModelEntry, account: &AccountId, reach: &Reach) -> Option<ModelCard> {
    let capabilities = crate::catalog::capabilities_of(entry);
    if capabilities.is_empty() {
        return None;
    }
    Some(ModelCard {
        account: account.clone(),
        model: ModelId::parse(&entry.id.0).ok()?,
        locality: Where::Cloud { region: None },
        billing: Billing::Metered(price_of(reach)),
        capabilities,
    })
}

/// The hosted models `accounts` (an app's verdicts) reach: per entry the best reach among the
/// granted accounts; failing that, among the ones that ask, then the denied.
pub fn remote_models(entries: &[ModelEntry], accounts: &[AccountVerdict]) -> Vec<RemoteModel> {
    let known = known_providers(entries);
    let resolved: Vec<(AccountVerdict, ProviderId)> = accounts
        .iter()
        .filter_map(|one| Some((one.clone(), provider_of(one, &known)?)))
        .collect();
    let providers = |wanted: Wanted| -> Vec<ProviderId> {
        resolved
            .iter()
            .filter(|(one, _)| wanted(&one.verdict))
            .map(|(_, provider)| provider.clone())
            .collect()
    };
    let tiers: [(Wanted, Vec<ProviderId>); 3] = [
        (is_granted, providers(is_granted)),
        (is_ask, providers(is_ask)),
        (is_denied, providers(is_denied)),
    ];
    entries
        .iter()
        .filter_map(|entry| {
            tiers.iter().find_map(|(wanted, granted)| {
                let reach = reachable(entry, granted, &WIRES)?;
                let account = account_with(&resolved, &reach.provider, *wanted)?;
                Some(RemoteModel {
                    card: card_of(entry, &account.account, reach)?,
                    entry: entry.clone(),
                    reach: reach.clone(),
                    verdict: account.verdict.clone(),
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
