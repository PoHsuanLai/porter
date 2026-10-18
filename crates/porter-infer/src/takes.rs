//! What a model takes in, in the words a person reads beside it in a model list. It is worked out
//! from the model's capabilities (the catalogue's for a hosted model, the card's for one that
//! runs on a computer), never guessed from its name; a model whose capabilities say nothing about
//! text is [`Takes::Unknown`] and gets no words.

use porter_core::Capability;
use porter_core::capability::LlmFeature;
use serde::{Deserialize, Serialize};

/// What a model reads or hears, besides how it answers.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Takes {
    /// Nothing says what it takes in.
    #[default]
    Unknown,
    /// Text only.
    TextOnly,
    /// Text and pictures.
    TextAndImages,
    /// Text and sound.
    TextAndAudio,
    /// Text, pictures and sound.
    TextImagesAndAudio,
}

/// Whether a model can look at a picture, such as a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sight {
    /// It reads images.
    Sees,
    /// It does not.
    Blind,
    /// Nothing says.
    Unknown,
}

/// What the capabilities of a model add up to, before they are named.
#[derive(Debug, Clone, Copy, Default)]
struct Found {
    text: bool,
    images: bool,
    audio: bool,
}

impl Found {
    fn with(self, capability: &Capability) -> Self {
        match capability {
            Capability::Llm(llm) => Self {
                text: true,
                images: self.images || llm.features.contains(&LlmFeature::Vision),
                audio: self.audio || llm.features.contains(&LlmFeature::AudioIn),
            },
            // A computer-use model works from screenshots.
            Capability::ComputerUse(_) => Self {
                text: true,
                images: true,
                ..self
            },
            _ => self,
        }
    }
}

impl Takes {
    /// What a model with these capabilities takes in.
    pub fn of(capabilities: &[Capability]) -> Self {
        let found = capabilities
            .iter()
            .fold(Found::default(), |found, one| found.with(one));
        match (found.text, found.images, found.audio) {
            (false, _, _) => Takes::Unknown,
            (true, false, false) => Takes::TextOnly,
            (true, true, false) => Takes::TextAndImages,
            (true, false, true) => Takes::TextAndAudio,
            (true, true, true) => Takes::TextImagesAndAudio,
        }
    }

    /// Whether it can look at a screenshot.
    pub fn sight(self) -> Sight {
        match self {
            Takes::Unknown => Sight::Unknown,
            Takes::TextOnly | Takes::TextAndAudio => Sight::Blind,
            Takes::TextAndImages | Takes::TextImagesAndAudio => Sight::Sees,
        }
    }

    /// The words beside the model in a list; none when nothing says.
    pub fn words(self) -> Option<&'static str> {
        match self {
            Takes::Unknown => None,
            Takes::TextOnly => Some("Text only"),
            Takes::TextAndImages => Some("Reads text and images"),
            Takes::TextAndAudio => Some("Reads text, hears audio"),
            Takes::TextImagesAndAudio => Some("Reads text and images, hears audio"),
        }
    }

    /// The name on the bus (the serde form).
    pub fn slug(self) -> &'static str {
        match self {
            Takes::Unknown => "unknown",
            Takes::TextOnly => "text_only",
            Takes::TextAndImages => "text_and_images",
            Takes::TextAndAudio => "text_and_audio",
            Takes::TextImagesAndAudio => "text_images_and_audio",
        }
    }

    /// The value a bus name stands for; a name this version does not know reads as unknown.
    pub fn from_slug(slug: &str) -> Self {
        [
            Takes::TextOnly,
            Takes::TextAndImages,
            Takes::TextAndAudio,
            Takes::TextImagesAndAudio,
        ]
        .into_iter()
        .find(|one| one.slug() == slug)
        .unwrap_or(Takes::Unknown)
    }

    /// Whether nothing is known (the wire leaves the field out then).
    pub fn is_unknown(&self) -> bool {
        *self == Takes::Unknown
    }
}

/// Said when a person's model cannot look at the screen but the work needs it.
pub fn cannot_see_screen(model: &str) -> String {
    format!(
        "{model} can't see the screen, so it can't use apps for you. Choose a model that reads images."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::Tokens;
    use porter_core::capability::{LlmCap, LlmWire};

    fn llm(features: &[LlmFeature]) -> Capability {
        Capability::Llm(LlmCap {
            features: features.iter().copied().collect(),
            context: Tokens(8192),
            max_output: Tokens(1024),
            wire: LlmWire::ChatCompletions,
        })
    }

    #[test]
    fn a_model_is_named_by_what_its_capabilities_take() {
        let cases = [
            (vec![], Takes::Unknown),
            (vec![llm(&[LlmFeature::Chat])], Takes::TextOnly),
            (
                vec![llm(&[LlmFeature::Chat, LlmFeature::Vision])],
                Takes::TextAndImages,
            ),
            (
                vec![llm(&[LlmFeature::AudioIn])],
                Takes::TextAndAudio,
            ),
            (
                vec![llm(&[LlmFeature::Vision, LlmFeature::AudioIn])],
                Takes::TextImagesAndAudio,
            ),
        ];
        for (capabilities, expected) in cases {
            assert_eq!(Takes::of(&capabilities), expected, "{capabilities:?}");
        }
    }

    #[test]
    fn only_a_model_that_reads_images_can_see_the_screen() {
        assert_eq!(Takes::TextOnly.sight(), Sight::Blind);
        assert_eq!(Takes::TextAndAudio.sight(), Sight::Blind);
        assert_eq!(Takes::TextAndImages.sight(), Sight::Sees);
        assert_eq!(Takes::Unknown.sight(), Sight::Unknown);
    }

    #[test]
    fn the_bus_name_round_trips_and_an_unknown_name_reads_as_unknown() {
        for takes in [
            Takes::TextOnly,
            Takes::TextAndImages,
            Takes::TextAndAudio,
            Takes::TextImagesAndAudio,
        ] {
            assert_eq!(Takes::from_slug(takes.slug()), takes);
            assert_eq!(
                serde_json::to_string(&takes).expect("json"),
                format!("\"{}\"", takes.slug())
            );
        }
        assert_eq!(Takes::from_slug("video"), Takes::Unknown);
        assert_eq!(Takes::Unknown.words(), None);
    }
}
