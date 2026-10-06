//! Slots: the jobs a person picks a model for (capabilities.md section 2). A slot is a capability
//! signature; any catalogue entry that satisfies it is listed there, so one model may sit in
//! several slots. The settings key is `ai.model.<slot>.<tier>`.
//!
//! `Slot` replaces `AiKind`. Old settings files and wire frames name the old kinds (`llm`,
//! `speech_in`, `speech_out`); they still read, as serde aliases, and map to their slot. The
//! deprecated [`AiKind`] alias and its variant-named constants stay until docket, almanac and cua
//! have moved.

use porter_core::Tier;
use serde::{Deserialize, Serialize};

/// The jobs a user maps models to. The slugs are the serde form and the settings-key segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// Text in, text out (with tools): language models. Was `AiKind::Llm`.
    #[serde(alias = "llm")]
    Text,
    /// Audio in, text out: speech to text. Was `AiKind::SpeechIn`.
    #[serde(alias = "speech_in")]
    VoiceIn,
    /// Text in, audio out: text to speech. Was `AiKind::SpeechOut`.
    #[serde(alias = "speech_out")]
    VoiceOut,
    /// Image in, text out: a model that reads or describes pictures.
    ImageIn,
    /// Image in, actions out: computer-use models.
    ComputerUse,
    /// Text in, vector out: embedding models.
    Embeddings,
    /// Image generators.
    ImageGen,
    /// Rerankers.
    Rerank,
}

/// The old name of [`Slot`].
#[deprecated(
    note = "use `Slot`; `AiKind::Llm` is `Slot::Text`, `SpeechIn` is `VoiceIn`, `SpeechOut` is `VoiceOut`"
)]
pub type AiKind = Slot;

/// The old variant names, so `AiKind::Llm` keeps compiling through the alias period.
#[allow(non_upper_case_globals)]
impl Slot {
    /// `Slot::Text`.
    pub const Llm: Slot = Slot::Text;
    /// `Slot::VoiceIn`.
    pub const SpeechIn: Slot = Slot::VoiceIn;
    /// `Slot::VoiceOut`.
    pub const SpeechOut: Slot = Slot::VoiceOut;
}

impl Slot {
    /// Every slot, in the order the picker lists them.
    pub const ALL: [Slot; 8] = [
        Slot::Text,
        Slot::VoiceIn,
        Slot::VoiceOut,
        Slot::ImageIn,
        Slot::ComputerUse,
        Slot::Embeddings,
        Slot::ImageGen,
        Slot::Rerank,
    ];

    /// The slot's stable slug: its serde form and its settings-key segment.
    pub fn slug(self) -> &'static str {
        match self {
            Slot::Text => "text",
            Slot::VoiceIn => "voice_in",
            Slot::VoiceOut => "voice_out",
            Slot::ImageIn => "image_in",
            Slot::ComputerUse => "computer_use",
            Slot::Embeddings => "embeddings",
            Slot::ImageGen => "image_gen",
            Slot::Rerank => "rerank",
        }
    }

    /// The slot a slug names, the old kind slugs (`llm`, `speech_in`, `speech_out`) included.
    pub fn from_slug(slug: &str) -> Option<Slot> {
        Slot::ALL
            .into_iter()
            .find(|slot| slot.slug() == slug)
            .or(match slug {
                "llm" => Some(Slot::Text),
                "speech_in" => Some(Slot::VoiceIn),
                "speech_out" => Some(Slot::VoiceOut),
                _ => None,
            })
    }

    /// The old kind slug this slot was written under, if it had one.
    pub fn legacy_slug(self) -> Option<&'static str> {
        match self {
            Slot::Text => Some("llm"),
            Slot::VoiceIn => Some("speech_in"),
            Slot::VoiceOut => Some("speech_out"),
            _ => None,
        }
    }

    /// Whether `slug` is an old kind slug rather than the slot's own.
    pub fn is_legacy_slug(slug: &str) -> bool {
        matches!(slug, "llm" | "speech_in" | "speech_out")
    }

    /// The settings key for the model chosen at `tier` (`ai.model.voice_in.balanced`).
    pub fn setting_key(self, tier: Tier) -> String {
        let tier = match tier {
            Tier::Fast => "fast",
            Tier::Balanced => "balanced",
            Tier::Best => "best",
        };
        format!("ai.model.{}.{tier}", self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_kinds_map_to_their_slots() {
        let cases = [
            ("llm", Slot::Text),
            ("speech_in", Slot::VoiceIn),
            ("speech_out", Slot::VoiceOut),
            ("computer_use", Slot::ComputerUse),
            ("embeddings", Slot::Embeddings),
            ("image_gen", Slot::ImageGen),
            ("rerank", Slot::Rerank),
            ("text", Slot::Text),
            ("image_in", Slot::ImageIn),
        ];
        for (slug, slot) in cases {
            assert_eq!(Slot::from_slug(slug), Some(slot), "{slug}");
            let json = serde_json::Value::String(slug.to_owned());
            assert_eq!(
                serde_json::from_value::<Slot>(json).ok(),
                Some(slot),
                "{slug}"
            );
        }
        assert_eq!(Slot::from_slug("tv"), None);
    }

    #[test]
    fn slots_write_their_own_slug_and_keys() {
        for slot in Slot::ALL {
            assert_eq!(
                serde_json::to_value(slot).expect("json"),
                serde_json::Value::String(slot.slug().to_owned())
            );
            assert_eq!(Slot::from_slug(slot.slug()), Some(slot));
            assert!(!Slot::is_legacy_slug(slot.slug()));
        }
        assert_eq!(Slot::Text.setting_key(Tier::Best), "ai.model.text.best");
        assert_eq!(
            Slot::VoiceIn.setting_key(Tier::Fast),
            "ai.model.voice_in.fast"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn the_old_names_still_compile() {
        assert_eq!(AiKind::Llm, Slot::Text);
        assert_eq!(AiKind::SpeechIn, Slot::VoiceIn);
        assert_eq!(AiKind::SpeechOut, Slot::VoiceOut);
        assert_eq!(AiKind::Rerank, Slot::Rerank);
    }
}
