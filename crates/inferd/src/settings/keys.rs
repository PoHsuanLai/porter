//! The vocabulary of the rows: slugs, the classes, slots and tiers they range over, and the
//! `ai.model.<slot>.<tier>` paths (the old `<kind>` segments still read).

use porter_core::{DataClass, HelperId, Tier};
use porter_infer::{Chooser, Slot};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The slots a model serves, from its capabilities: moved to `porter_router::router`.
pub use porter_router::router::slots_of;

/// Every data class, one `ai.floor.<class>` row each. `class_is_listed` keeps this complete.
pub const CLASSES: [DataClass; 13] = [
    DataClass::AppOwn,
    DataClass::Mail,
    DataClass::Calendar,
    DataClass::Contacts,
    DataClass::Tasks,
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

/// `ai.auto.pool.<slot>`: the models Automatic chooses among for the slot.
pub fn pool_path(slot: Slot) -> String {
    format!("ai.auto.pool.{}", slot.slug())
}

/// `ai.auto.pool.<slot>.helper.<id>`: the models Automatic chooses among for the slot when
/// `chooser` asks. The assistant's is `pool_path`'s, unchanged. In `inferd.toml` a helper's key
/// is the quoted tail (`"text.helper.<id>" = [...]` under `[ai.auto.pool]`), since a table cannot
/// share a name with the assistant's list. Only the round trip reads it until a writer of
/// helpers' pools lands.
#[cfg(test)]
pub fn pool_path_of(slot: Slot, chooser: &Chooser) -> String {
    match chooser {
        Chooser::Assistant => pool_path(slot),
        Chooser::Helper(helper) => format!("{}.helper.{helper}", pool_path(slot)),
    }
}

/// The slot and chooser the tail of an `ai.auto.pool.` path names: `<slot>` for the assistant,
/// `<slot>.helper.<id>` for a helper (the id is everything after `helper.`, dots included).
pub fn parse_pool_key(tail: &str) -> Option<(Slot, Chooser)> {
    match tail.split_once('.') {
        None => Some((Slot::from_slug(tail)?, Chooser::Assistant)),
        Some((slot, rest)) => {
            let helper = HelperId::parse(rest.strip_prefix("helper.")?).ok()?;
            Some((Slot::from_slug(slot)?, Chooser::Helper(helper)))
        }
    }
}

/// The slot an `ai.auto.pool.<slot>` path names.
pub fn parse_pool_path(path: &str) -> Option<Slot> {
    Slot::from_slug(path.strip_prefix("ai.auto.pool.")?)
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
                | DataClass::Tasks
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

    #[test]
    fn pool_keys_name_the_assistant_or_a_helper() {
        let helper = Chooser::Helper(HelperId::parse("h.1").expect("id"));
        for (tail, expected) in [
            ("text", Some((Slot::Text, Chooser::Assistant))),
            ("text.helper.h.1", Some((Slot::Text, helper.clone()))),
            ("text.helper.", None),
            ("text.other.h-1", None),
            ("nothing.helper.h-1", None),
        ] {
            assert_eq!(parse_pool_key(tail), expected, "{tail}");
        }
        assert_eq!(
            pool_path_of(Slot::Text, &Chooser::Assistant),
            "ai.auto.pool.text"
        );
        assert_eq!(
            pool_path_of(Slot::Text, &helper),
            "ai.auto.pool.text.helper.h.1"
        );
        assert_eq!(parse_pool_path("ai.auto.pool.text.helper.h.1"), None);
    }
}
