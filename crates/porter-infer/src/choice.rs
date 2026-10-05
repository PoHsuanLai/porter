//! The model picker's data (design/31 §5.5): which model the user mapped to each kind and tier,
//! and the plain list a settings page draws. The list ranks nothing: no model is marked best or
//! recommended, and a model the user mapped to a tier is only marked as theirs.

use crate::pick::{AutoMode, Pick};
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

/// The label a settings page draws for a tier. The key segment stays `best` (a name for a slot,
/// kept for compatibility), but the label says what the slot is for and ranks nothing.
pub fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Fast => "Fast",
        Tier::Balanced => "Balanced",
        Tier::Best => "Demanding",
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

/// A kind and tier the user set to "auto": settings `ai.model.<kind>.<tier>` = `"auto"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AutoRow {
    /// The kind.
    pub kind: AiKind,
    /// The tier.
    pub tier: Tier,
    /// What Automatic does for it.
    pub mode: AutoMode,
}

/// Every choice the user made: settings `ai.model.<kind>.<tier>` = `<account>/<model>` or `"auto"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierMap {
    /// The rows naming a model; at most one per kind and tier.
    pub rows: Vec<TierRow>,
    /// The rows set to "auto"; at most one per kind and tier.
    #[serde(default)]
    pub autos: Vec<AutoRow>,
}

impl TierMap {
    /// The person's pick for `kind` and `tier`, or `None` when the row is empty (the catalogue's
    /// own choice, as before). A row that names a model wins over an "auto" row for the same
    /// slot: a named model is never overridden, so a file that says both stays with the name.
    pub fn pick(&self, kind: AiKind, tier: Tier) -> Option<Pick> {
        let named = self
            .rows
            .iter()
            .find(|row| row.kind == kind && row.tier == tier)
            .map(|row| Pick::Named(row.model.clone()));
        named.or_else(|| {
            self.autos
                .iter()
                .find(|row| row.kind == kind && row.tier == tier)
                .map(|row| Pick::Auto(row.mode))
        })
    }
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
    let mut rows: Vec<PickerRow> = cards
        .iter()
        .filter(|card| card.kind == kind)
        .map(|card| row_of(card, map))
        .collect();
    // `sort_by_key` is stable: equal keys keep the catalog's order.
    rows.sort_by_key(|row| (closeness(&row.locality), soonness(row.readiness)));
    rows
}

fn row_of(card: &PickerInput, map: &TierMap) -> PickerRow {
    let chosen_for = [Tier::Fast, Tier::Balanced, Tier::Best]
        .into_iter()
        .filter(|tier| tier_choice(map, card.kind, *tier, &card.model) == TierChoice::Chosen)
        .collect();
    PickerRow {
        model: card.model.clone(),
        label: card.label.clone(),
        kind: card.kind,
        locality: card.locality.clone(),
        billing: card.billing.clone(),
        readiness: card.readiness,
        fit: card.fit,
        licence: card.licence,
        chosen_for,
    }
}

/// This computer, then the user's other machines, then the cloud; regions do not order.
fn closeness(locality: &Locality) -> u8 {
    match locality {
        Locality::OnDevice => 0,
        Locality::LocalNetwork => 1,
        Locality::Cloud { .. } => 2,
    }
}

/// How soon the model can answer; a download in progress sorts with the downloadable.
fn soonness(readiness: Readiness) -> u8 {
    match readiness {
        Readiness::Ready => 0,
        Readiness::Loading => 1,
        Readiness::Loadable => 2,
        Readiness::Downloading(_) | Readiness::Downloadable => 3,
        Readiness::Unavailable => 4,
    }
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
            autos: vec![],
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

    fn card(name: &str, kind: AiKind, locality: Locality, readiness: Readiness) -> PickerInput {
        PickerInput {
            model: model("local", name),
            label: name.to_owned(),
            kind,
            locality,
            billing: Billing::Free,
            readiness,
            fit: Fit::Fits,
            licence: LicenceClass::Open,
        }
    }

    fn names(rows: &[PickerRow]) -> Vec<&str> {
        rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn picker_rows_filter_then_order_by_closeness_then_readiness() {
        use Readiness::*;
        let cloud = |region: Option<&str>| Locality::Cloud {
            region: region.map(|r| porter_core::Region(r.to_owned())),
        };
        let cards = [
            card("far-ready", AiKind::Llm, cloud(Some("eu-west-1")), Ready),
            card("speech", AiKind::SpeechIn, Locality::OnDevice, Ready),
            card("near-down", AiKind::Llm, Locality::OnDevice, Downloadable),
            card("near-stopped", AiKind::Llm, Locality::OnDevice, Loadable),
            card("lan", AiKind::Llm, Locality::LocalNetwork, Ready),
            card("near-ready-a", AiKind::Llm, Locality::OnDevice, Ready),
            card("near-ready-b", AiKind::Llm, Locality::OnDevice, Ready),
            card("far-ready-2", AiKind::Llm, cloud(None), Ready),
            card("near-loading", AiKind::Llm, Locality::OnDevice, Loading),
            card("near-gone", AiKind::Llm, Locality::OnDevice, Unavailable),
        ];
        let cases = [
            (
                AiKind::Llm,
                vec![
                    "near-ready-a",
                    "near-ready-b",
                    "near-loading",
                    "near-stopped",
                    "near-down",
                    "near-gone",
                    "lan",
                    "far-ready",
                    "far-ready-2",
                ],
            ),
            (AiKind::SpeechIn, vec!["speech"]),
            (AiKind::Rerank, vec![]),
        ];
        for (kind, expected) in cases {
            let rows = picker_rows(kind, &cards, &TierMap::default());
            assert_eq!(names(&rows), expected, "{kind:?}");
        }
    }

    #[test]
    fn picker_rows_mark_the_tiers_the_user_mapped() {
        let a = card("a", AiKind::Llm, Locality::OnDevice, Readiness::Ready);
        let map = TierMap {
            rows: vec![
                TierRow {
                    kind: AiKind::Llm,
                    tier: Tier::Fast,
                    model: a.model.clone(),
                },
                TierRow {
                    kind: AiKind::Llm,
                    tier: Tier::Best,
                    model: a.model.clone(),
                },
                TierRow {
                    kind: AiKind::SpeechIn,
                    tier: Tier::Balanced,
                    model: a.model.clone(),
                },
            ],
            autos: vec![],
        };
        let rows = picker_rows(AiKind::Llm, &[a], &map);
        assert_eq!(rows[0].chosen_for, BTreeSet::from([Tier::Fast, Tier::Best]));
    }

    #[test]
    fn picker_rows_carry_no_ranking_words() {
        let cards = [
            card("a", AiKind::Llm, Locality::OnDevice, Readiness::Ready),
            card(
                "b",
                AiKind::Llm,
                Locality::LocalNetwork,
                Readiness::Loadable,
            ),
        ];
        let json = serde_json::to_value(picker_rows(AiKind::Llm, &cards, &TierMap::default()))
            .expect("serializes");
        let mut keys = Vec::new();
        collect_keys(&json, &mut keys);
        for word in [
            "rank",
            "score",
            "recommended",
            "best",
            "top",
            "preferred",
            "default",
        ] {
            assert!(
                !keys.iter().any(|k| k.contains(word)),
                "a row field names `{word}`: {keys:?}"
            );
        }
    }

    fn collect_keys(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    collect_keys(v, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| collect_keys(v, out)),
            _ => {}
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

    #[test]
    fn the_best_slot_keeps_its_key_and_shows_another_label() {
        assert_eq!(AiKind::Llm.setting_key(Tier::Best), "ai.model.llm.best");
        let labels: Vec<_> = [Tier::Fast, Tier::Balanced, Tier::Best]
            .into_iter()
            .map(tier_label)
            .collect();
        assert_eq!(labels, ["Fast", "Balanced", "Demanding"]);
        for label in labels {
            for word in ["best", "recommended", "optimal", "smart", "premium"] {
                assert!(!label.to_lowercase().contains(word), "{label}");
            }
        }
    }

    #[test]
    fn the_tier_map_says_named_auto_or_nothing_and_a_name_wins() {
        let a = model("local", "a");
        let map = TierMap {
            rows: vec![
                TierRow {
                    kind: AiKind::Llm,
                    tier: Tier::Fast,
                    model: a.clone(),
                },
                TierRow {
                    kind: AiKind::Llm,
                    tier: Tier::Best,
                    model: a.clone(),
                },
            ],
            autos: vec![
                AutoRow {
                    kind: AiKind::Llm,
                    tier: Tier::Balanced,
                    mode: AutoMode::WarmFirst,
                },
                AutoRow {
                    kind: AiKind::Llm,
                    tier: Tier::Best,
                    mode: AutoMode::WarmFirst,
                },
            ],
        };
        assert_eq!(
            map.pick(AiKind::Llm, Tier::Fast),
            Some(Pick::Named(a.clone()))
        );
        assert_eq!(
            map.pick(AiKind::Llm, Tier::Balanced),
            Some(Pick::Auto(AutoMode::WarmFirst))
        );
        assert_eq!(map.pick(AiKind::Llm, Tier::Best), Some(Pick::Named(a)));
        assert_eq!(map.pick(AiKind::Embeddings, Tier::Fast), None);
    }

    #[test]
    fn a_tier_map_written_before_auto_rows_still_reads() {
        let old = r#"{"rows":[]}"#;
        let map: TierMap = serde_json::from_str(old).expect("reads");
        assert_eq!(map, TierMap::default());
    }
}
