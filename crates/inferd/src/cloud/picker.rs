//! The hosted models as the model picker lists them: every curated remote entry that fits a
//! slot, grouped by company, whether or not the person has an account for it. An entry no
//! account reaches is listed, unavailable with the reason "Add an account to use" (the schema's
//! `unavailable`, which Settings greys and the module refuses to set), so the picker is company then
//! model regardless of accounts. The label is the model's name alone, as a Mac menu shows it; the
//! company is the group it sits under.

use super::accountd::AccountVerdict;
use super::models::{WIRES, known_providers, provider_of};
use crate::engines::CLOUD_ACCOUNT;
use model_catalog::{ModelEntry, reachable, slot_members};

/// Why a model nobody can reach yet cannot be picked (the schema's `unavailable` reason; the
/// Settings app shows it and greys the choice).
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
        "deepseek" => "DeepSeek".to_owned(),
        "gemini" => "Google".to_owned(),
        "kimi" => "Moonshot".to_owned(),
        "muse" => "Meta".to_owned(),
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

/// The catalogue's slot for porter's: the two have the same names, but the catalogue has no
/// `image_gen` or `rerank` yet, so those list no hosted model.
pub fn catalog_slot(slot: porter_infer::Slot) -> Option<model_catalog::Slot> {
    use model_catalog::Slot as C;
    use porter_infer::Slot as P;
    match slot {
        P::Text => Some(C::Text),
        P::VoiceIn => Some(C::VoiceIn),
        P::VoiceOut => Some(C::VoiceOut),
        P::ImageIn => Some(C::ImageIn),
        P::ComputerUse => Some(C::ComputerUse),
        P::Embeddings => Some(C::Embeddings),
        P::ImageGen | P::Rerank => None,
    }
}

/// The name a person reads for a model: the catalogue's label without a trailing tier note such
/// as "(contributor)".
fn plain_name(label: &str) -> &str {
    match label.rfind(" (") {
        Some(at) if label.ends_with(')') => label[..at].trim_end(),
        _ => label,
    }
}

/// The hosted models of `slot`, grouped by company (companies in the order the
/// catalogue first names them, models in catalogue order), marked by `accounts`: every account of
/// the person's, whatever the app's grant says.
pub fn choices(
    entries: &[ModelEntry],
    accounts: &[AccountVerdict],
    slot: porter_infer::Slot,
) -> Vec<Choice> {
    let Some(slot) = catalog_slot(slot) else {
        return Vec::new();
    };
    let known = known_providers(entries);
    let held: Vec<_> = accounts
        .iter()
        .filter_map(|one| provider_of(one, &known))
        .collect();
    let mut grouped: Vec<(String, Vec<Choice>)> = Vec::new();
    for entry in slot_members(slot, entries, &[]) {
        let Some(reach) = reachable_or_not(entry, &held) else {
            continue;
        };
        let company = company_of(&entry.family.0);
        let label = plain_name(&entry.label).to_owned();
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
