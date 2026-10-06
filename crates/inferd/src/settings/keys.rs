//! The vocabulary of the rows: slugs, the classes, slots and tiers they range over, and the
//! `ai.model.<slot>.<tier>` paths (the old `<kind>` segments still read).

use porter_core::capability::{LlmFeature, SpeechMode};
use porter_core::{Capability, DataClass, Tier};
use porter_infer::{ModelCard, Slot};
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

/// Every slot, one `ai.model.<slot>.<tier>` row each per tier.
pub const SLOTS: [Slot; 8] = Slot::ALL;

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

/// `ai.model.<slot>.<tier>`.
pub fn model_path(slot: Slot, tier: Tier) -> String {
    slot.setting_key(tier)
}

/// The slot and tier an `ai.model.<slot>.<tier>` path names. An old kind segment (`llm`,
/// `speech_in`, `speech_out`) names the slot it became.
pub fn parse_model_path(path: &str) -> Option<(Slot, Tier)> {
    let (slot, tier) = path.strip_prefix("ai.model.")?.split_once('.')?;
    Some((Slot::from_slug(slot)?, from_slug(tier)?))
}

/// The slots a model serves, from its capabilities: a language model is in `text`, and in
/// `image_in` when it takes images and `voice_in` when it takes audio; a computer-use model reads
/// images too.
pub fn slots_of(card: &ModelCard) -> Vec<Slot> {
    let mut slots: Vec<Slot> = card
        .capabilities
        .iter()
        .flat_map(|capability| match capability {
            Capability::Llm(llm) => {
                let extra = [
                    (LlmFeature::Vision, Slot::ImageIn),
                    (LlmFeature::AudioIn, Slot::VoiceIn),
                ]
                .into_iter()
                .filter(|(feature, _)| llm.features.contains(feature))
                .map(|(_, slot)| slot);
                std::iter::once(Slot::Text).chain(extra).collect()
            }
            Capability::ComputerUse(_) => vec![Slot::ComputerUse, Slot::ImageIn],
            Capability::Embeddings(_) => vec![Slot::Embeddings],
            Capability::ImageGen(_) => vec![Slot::ImageGen],
            Capability::Rerank(_) => vec![Slot::Rerank],
            Capability::Speech(speech) => [
                (SpeechMode::Stt, Slot::VoiceIn),
                (SpeechMode::Tts, Slot::VoiceOut),
            ]
            .into_iter()
            .filter(|(mode, _)| speech.modes.contains(mode))
            .map(|(_, slot)| slot)
            .collect(),
            _ => Vec::new(),
        })
        .collect();
    slots.sort();
    slots.dedup();
    slots
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
    fn slots_are_listed() {
        for slot in SLOTS {
            match slot {
                Slot::Text
                | Slot::VoiceIn
                | Slot::VoiceOut
                | Slot::ImageIn
                | Slot::ComputerUse
                | Slot::Embeddings
                | Slot::ImageGen
                | Slot::Rerank => {}
            }
        }
        assert_eq!(SLOTS.len(), 8);
    }

    #[test]
    fn model_paths_round_trip_and_old_kinds_map_to_slots() {
        for slot in SLOTS {
            for tier in TIERS {
                assert_eq!(
                    parse_model_path(&model_path(slot, tier)),
                    Some((slot, tier))
                );
            }
        }
        let old = [
            ("ai.model.llm.fast", Slot::Text, Tier::Fast),
            ("ai.model.speech_in.balanced", Slot::VoiceIn, Tier::Balanced),
            ("ai.model.speech_out.best", Slot::VoiceOut, Tier::Best),
        ];
        for (path, slot, tier) in old {
            assert_eq!(parse_model_path(path), Some((slot, tier)), "{path}");
        }
        for bad in [
            "ai.model.text",
            "ai.model.text.huge",
            "ai.model.tv.fast",
            "ai.auto.mode",
        ] {
            assert_eq!(parse_model_path(bad), None, "{bad}");
        }
    }
}
