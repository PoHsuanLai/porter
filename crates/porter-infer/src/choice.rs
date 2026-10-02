//! The model picker's data (design/31 §5.5): which model the user mapped to each kind and tier,
//! and the plain list a settings page draws. The list ranks nothing: no model is marked best or
//! recommended, and a model the user mapped to a tier is only marked as theirs.

use crate::readiness::Readiness;
use crate::route::TierChoice;
use porter_core::{AccountId, Billing, Locality, ModelId, Tier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One model on one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModelRef {
    /// The account that serves it.
    pub account: AccountId,
    /// Its id.
    pub model: ModelId,
}

/// The AI kinds a user maps models to: `CapabilityKind`'s AI subset, with speech split by
/// direction because a speech-to-text model and a text-to-speech model are chosen separately.
///
/// `SpeechIn` stands for `Need::Speech` with `{Stt}`, `SpeechOut` for `{Tts}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiKind {
    /// Language models.
    Llm,
    /// Computer-use models.
    ComputerUse,
    /// Embedding models.
    Embeddings,
    /// Speech to text.
    SpeechIn,
    /// Text to speech.
    SpeechOut,
    /// Image generators.
    ImageGen,
    /// Rerankers.
    Rerank,
}

impl AiKind {
    /// The kind's stable slug: its serde form and its settings-key segment.
    pub fn slug(self) -> &'static str {
        match self {
            AiKind::Llm => "llm",
            AiKind::ComputerUse => "computer_use",
            AiKind::Embeddings => "embeddings",
            AiKind::SpeechIn => "speech_in",
            AiKind::SpeechOut => "speech_out",
            AiKind::ImageGen => "image_gen",
            AiKind::Rerank => "rerank",
        }
    }

    /// The settings key for the model chosen at `tier` (`ai.model.speech_in.balanced`).
    pub fn setting_key(self, tier: Tier) -> String {
        let tier = match tier {
            Tier::Fast => "fast",
            Tier::Balanced => "balanced",
            Tier::Best => "best",
        };
        format!("ai.model.{}.{tier}", self.slug())
    }
}

/// The user's choice for one kind and tier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TierRow {
    /// The kind.
    pub kind: AiKind,
    /// The tier.
    pub tier: Tier,
    /// The model chosen.
    pub model: ModelRef,
}

/// Every choice the user made: settings `ai.model.<kind>.<tier>` = `<account>/<model>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierMap {
    /// The rows; at most one per kind and tier.
    pub rows: Vec<TierRow>,
}

/// Whether `model` is the user's choice for `kind` at `tier`. Feeds `route` unchanged.
pub fn tier_choice(map: &TierMap, kind: AiKind, tier: Tier, model: &ModelRef) -> TierChoice {
    let chosen = map
        .rows
        .iter()
        .any(|row| row.kind == kind && row.tier == tier && row.model == *model);
    if chosen {
        TierChoice::Chosen
    } else {
        TierChoice::Other
    }
}

/// Whether a model fits in memory now (the supervisor's budget verdict, as a word).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// It fits beside what is loaded.
    Fits,
    /// Something loaded must be unloaded first.
    NeedsEviction,
    /// Some layers would run on the CPU.
    NeedsOffload,
    /// It cannot fit on this computer.
    TooLarge,
}

/// How a model may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenceClass {
    /// Open weights, commercial use allowed.
    Open,
    /// Non-commercial weights: listed, never chosen for the user.
    NonCommercial,
    /// A provider's hosted model.
    Proprietary,
}

/// What the picker knows about one model before it draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerInput {
    /// The model.
    pub model: ModelRef,
    /// The name to show.
    pub label: String,
    /// Which kind it serves.
    pub kind: AiKind,
    /// Where it runs.
    pub locality: Locality,
    /// What it costs.
    pub billing: Billing,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// Whether it fits in memory.
    pub fit: Fit,
    /// How it may be used.
    pub licence: LicenceClass,
}

/// One row of the plain list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerRow {
    /// The model.
    pub model: ModelRef,
    /// The name to show.
    pub label: String,
    /// Which kind it serves.
    pub kind: AiKind,
    /// Where it runs.
    pub locality: Locality,
    /// What it costs.
    pub billing: Billing,
    /// Whether it can answer now.
    pub readiness: Readiness,
    /// Whether it fits in memory.
    pub fit: Fit,
    /// How it may be used.
    pub licence: LicenceClass,
    /// The tiers the user mapped it to.
    pub chosen_for: BTreeSet<Tier>,
}

/// The plain list for `kind`: models of that kind only, this computer first, then ready before
/// loadable before downloadable, then the order the cards came in (the catalog's order). No
/// ranking by quality; a non-commercial row is listed and never mapped to a tier by itself.
pub fn picker_rows(kind: AiKind, cards: &[PickerInput], map: &TierMap) -> Vec<PickerRow> {
    let _ = (kind, cards, map);
    todo!("filter by kind, stable sort by (locality closeness, readiness), chosen_for from map")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(account: &str, model: &str) -> ModelRef {
        ModelRef {
            account: AccountId::parse(account).expect("account"),
            model: ModelId::parse(model).expect("model"),
        }
    }

    #[test]
    fn tier_choice_reads_the_map() {
        let nemotron = model("local", "nemotron");
        let map = TierMap {
            rows: vec![TierRow {
                kind: AiKind::SpeechIn,
                tier: Tier::Balanced,
                model: nemotron.clone(),
            }],
        };
        let cases = [
            (
                "the chosen model",
                AiKind::SpeechIn,
                Tier::Balanced,
                &nemotron,
                TierChoice::Chosen,
            ),
            (
                "another tier",
                AiKind::SpeechIn,
                Tier::Fast,
                &nemotron,
                TierChoice::Other,
            ),
            (
                "another kind",
                AiKind::SpeechOut,
                Tier::Balanced,
                &nemotron,
                TierChoice::Other,
            ),
            (
                "another model",
                AiKind::SpeechIn,
                Tier::Balanced,
                &model("local", "whisper"),
                TierChoice::Other,
            ),
        ];
        for (name, kind, tier, who, expected) in cases {
            assert_eq!(tier_choice(&map, kind, tier, who), expected, "{name}");
        }
    }

    #[test]
    fn setting_keys_name_kind_and_tier() {
        assert_eq!(
            AiKind::SpeechIn.setting_key(Tier::Fast),
            "ai.model.speech_in.fast"
        );
        assert_eq!(
            AiKind::ComputerUse.setting_key(Tier::Balanced),
            "ai.model.computer_use.balanced"
        );
    }
}
