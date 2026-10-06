//! The vocabulary of the rows: slugs, the classes, kinds and tiers they range over, and the
//! `ai.model.<kind>.<tier>` paths.

use porter_core::capability::SpeechMode;
use porter_core::{Capability, DataClass, Tier};
use porter_infer::{AiKind, ModelCard};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Every data class, one `ai.floor.<class>` row each. `class_is_listed` keeps this complete.
pub const CLASSES: [DataClass; 12] = [
    DataClass::AppOwn,
    DataClass::Mail,
    DataClass::Calendar,
    DataClass::Contacts,
    DataClass::Notes,
    DataClass::Files,
    DataClass::Photos,
    DataClass::Clipboard,
    DataClass::Screen,
    DataClass::Voice,
    DataClass::Prompt,
    DataClass::Public,
];

/// Every kind, one `ai.model.<kind>.<tier>` row each per tier.
pub const KINDS: [AiKind; 7] = [
    AiKind::Llm,
    AiKind::ComputerUse,
    AiKind::Embeddings,
    AiKind::SpeechIn,
    AiKind::SpeechOut,
    AiKind::ImageGen,
    AiKind::Rerank,
];

/// Every tier.
pub const TIERS: [Tier; 3] = [Tier::Fast, Tier::Balanced, Tier::Best];

/// The serde slug of a closed set's value.
pub fn slug_of<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The value a slug names.
pub fn from_slug<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// `ai.model.<kind>.<tier>`.
pub fn model_path(kind: AiKind, tier: Tier) -> String {
    kind.setting_key(tier)
}

/// The kind and tier an `ai.model.<kind>.<tier>` path names.
pub fn parse_model_path(path: &str) -> Option<(AiKind, Tier)> {
    let (kind, tier) = path.strip_prefix("ai.model.")?.split_once('.')?;
    Some((from_slug(kind)?, from_slug(tier)?))
}

/// The kinds a model serves, from its capabilities.
pub fn kinds_of(card: &ModelCard) -> Vec<AiKind> {
    let mut kinds: Vec<AiKind> = card
        .capabilities
        .iter()
        .flat_map(|capability| match capability {
            Capability::Llm(_) => vec![AiKind::Llm],
            Capability::ComputerUse(_) => vec![AiKind::ComputerUse],
            Capability::Embeddings(_) => vec![AiKind::Embeddings],
            Capability::ImageGen(_) => vec![AiKind::ImageGen],
            Capability::Rerank(_) => vec![AiKind::Rerank],
            Capability::Speech(speech) => [
                (SpeechMode::Stt, AiKind::SpeechIn),
                (SpeechMode::Tts, AiKind::SpeechOut),
            ]
            .into_iter()
            .filter(|(mode, _)| speech.modes.contains(mode))
            .map(|(_, kind)| kind)
            .collect(),
            _ => Vec::new(),
        })
        .collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_is_listed() {
        // A new `DataClass` breaks this match, and so the list above is made to follow it.
        for class in CLASSES {
            match class {
                DataClass::AppOwn
                | DataClass::Mail
                | DataClass::Calendar
                | DataClass::Contacts
                | DataClass::Notes
                | DataClass::Files
                | DataClass::Photos
                | DataClass::Clipboard
                | DataClass::Screen
                | DataClass::Voice
                | DataClass::Prompt
                | DataClass::Public => {}
            }
        }
        let mut slugs: Vec<String> = CLASSES.iter().map(slug_of).collect();
        slugs.dedup();
        assert_eq!(slugs.len(), 12);
    }

    #[test]
    fn kinds_are_listed() {
        for kind in KINDS {
            match kind {
                AiKind::Llm
                | AiKind::ComputerUse
                | AiKind::Embeddings
                | AiKind::SpeechIn
                | AiKind::SpeechOut
                | AiKind::ImageGen
                | AiKind::Rerank => {}
            }
        }
    }

    #[test]
    fn model_paths_round_trip() {
        for kind in KINDS {
            for tier in TIERS {
                assert_eq!(
                    parse_model_path(&model_path(kind, tier)),
                    Some((kind, tier))
                );
            }
        }
        for bad in [
            "ai.model.llm",
            "ai.model.llm.huge",
            "ai.model.tv.fast",
            "ai.auto.mode",
        ] {
            assert_eq!(parse_model_path(bad), None, "{bad}");
        }
    }
}
