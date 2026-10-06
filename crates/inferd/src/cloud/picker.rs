//! The hosted models as the model picker lists them: every curated remote entry that fits the
//! language slot, grouped by company, whether or not the person has an account for it. An entry no
//! account reaches is listed and says so ("Add an account to use"), so the picker is company then
//! model regardless of accounts. The label carries what a search and a tools filter read: the
//! company, and the capabilities the entry declares.

use super::accountd::AccountVerdict;
use super::models::{WIRES, known_providers, provider_of};
use crate::engines::CLOUD_ACCOUNT;
use model_catalog::{Modality, ModelEntry, Slot, reachable, slot_members};
use model_provider::ToolSupport;

/// What the label says of a model nobody can reach yet.
pub const NEEDS_ACCOUNT: &str = "Add an account to use";

/// One hosted model in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The row's value: `cloud/<entry id>`.
    pub value: String,
    /// The text of the menu item.
    pub label: String,
    /// The company the model is grouped under.
    pub company: String,
    /// Whether some account reaches it.
    pub available: Availability,
}

/// Whether the person can use a model now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// An account reaches it.
    Reachable,
    /// No account does.
    NeedsAccount,
}

/// The company a model family belongs to, as a person knows it.
pub fn company_of(family: &str) -> String {
    match family {
        "claude" => "Anthropic".to_owned(),
        "gemini" => "Google".to_owned(),
        "kimi" => "Moonshot".to_owned(),
        "gpt" => "OpenAI".to_owned(),
        other => {
            let mut letters = other.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default()
        }
    }
}

/// What an entry declares it can do, as the words a filter reads.
fn capabilities_of(entry: &ModelEntry) -> Vec<&'static str> {
    let caps = &entry.capabilities;
    let tools = caps
        .text_out
        .as_ref()
        .is_some_and(|text| text.tools != ToolSupport::Absent);
    let reasoning = caps
        .text_out
        .as_ref()
        .is_some_and(|text| text.reasoning == model_provider::Support::Present);
    [
        (tools, "tools"),
        (caps.inputs.contains(Modality::Image), "images"),
        (reasoning, "reasoning"),
    ]
    .into_iter()
    .filter_map(|(has, word)| has.then_some(word))
    .collect()
}

/// The hosted models of the language slot, grouped by company (companies in the order the
/// catalogue first names them, models in catalogue order), marked by `accounts`: every account of
/// the person's, whatever the app's grant says.
pub fn choices(entries: &[ModelEntry], accounts: &[AccountVerdict]) -> Vec<Choice> {
    let known = known_providers(entries);
    let held: Vec<_> = accounts
        .iter()
        .filter_map(|one| provider_of(one, &known))
        .collect();
    let mut grouped: Vec<(String, Vec<Choice>)> = Vec::new();
    for entry in slot_members(Slot::Text, entries, &[]) {
        let Some(reach) = reachable_or_not(entry, &held) else {
            continue;
        };
        let company = company_of(&entry.family.0);
        let mut words = vec![company.clone()];
        words.extend(capabilities_of(entry).into_iter().map(str::to_owned));
        let mut label = format!("{} ({})", entry.label, words.join(", "));
        if reach == Availability::NeedsAccount {
            label = format!("{label} - {NEEDS_ACCOUNT}");
        }
        let choice = Choice {
            value: format!("{CLOUD_ACCOUNT}/{}", entry.id.0),
            label,
            company: company.clone(),
            available: reach,
        };
        match grouped.iter_mut().find(|(name, _)| *name == company) {
            Some((_, group)) => group.push(choice),
            None => grouped.push((company, vec![choice])),
        }
    }
    grouped.into_iter().flat_map(|(_, group)| group).collect()
}

/// `None` for an entry that is not hosted.
fn reachable_or_not(
    entry: &ModelEntry,
    held: &[model_catalog::ProviderId],
) -> Option<Availability> {
    match entry.locality {
        model_catalog::Locality::OnDevice => None,
        model_catalog::Locality::Remote { .. } => Some(match reachable(entry, held, &WIRES) {
            Some(_) => Availability::Reachable,
            None => Availability::NeedsAccount,
        }),
    }
}

#[cfg(test)]
mod tests;
