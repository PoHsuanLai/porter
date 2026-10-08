//! A desktop-wide Space as accountd's registry of them keeps it (`spaces.json`,
//! `org.quire.Spaces1`): its id, the name the person typed, and a look porter does not read.

use crate::error::CoreError;
use crate::space::DesktopSpace;
use crate::units::UnixSeconds;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The longest Space name, in characters.
pub const SPACE_NAME_MAX_CHARS: usize = 64;
/// The longest look, in bytes of UTF-8.
pub const SPACE_LOOK_MAX_BYTES: usize = 1024;

/// A Space's name as the person typed it: not blank, at most [`SPACE_NAME_MAX_CHARS`]
/// characters, no control characters. porter adds no words of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SpaceName(String);

impl SpaceName {
    /// The name `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let ok = !text.trim().is_empty()
            && text.chars().count() <= SPACE_NAME_MAX_CHARS
            && !text.chars().any(char::is_control);
        match ok {
            true => Ok(Self(text.to_owned())),
            false => Err(CoreError::MalformedId {
                what: "space name",
                text: text.chars().take(SPACE_NAME_MAX_CHARS).collect(),
            }),
        }
    }

    /// The name's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SpaceName {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<SpaceName> for String {
    fn from(name: SpaceName) -> String {
        name.0
    }
}

impl fmt::Display for SpaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a Space looks (a colour, a symbol), as quire's Spaces kit writes it (compact JSON of its
/// `SpaceLook`). Opaque to porter: kept and handed back as it came, never parsed; only its
/// length is checked (at most [`SPACE_LOOK_MAX_BYTES`]). Empty is the kit's default look.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SpaceLook(String);

impl SpaceLook {
    /// The look `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        match text.len() <= SPACE_LOOK_MAX_BYTES {
            true => Ok(Self(text.to_owned())),
            false => Err(CoreError::MalformedId {
                what: "space look",
                text: format!("{} bytes", text.len()),
            }),
        }
    }

    /// The look's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SpaceLook {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<SpaceLook> for String {
    fn from(look: SpaceLook) -> String {
        look.0
    }
}

/// One desktop-wide Space.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DesktopSpaceRecord {
    /// Its id, minted once and kept across renames.
    pub id: DesktopSpace,
    /// What the person called it.
    pub name: SpaceName,
    /// How it looks, for quire's kit.
    pub look: SpaceLook,
    /// When it was made (or adopted).
    pub created: UnixSeconds,
}

/// What became of a desktop-wide Space (`Spaces1.Changed`'s `what`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceChange {
    /// It was made.
    Created,
    /// It has a new name.
    Renamed,
    /// It has a new look.
    Look,
    /// It is gone, and every grant scoped to it ended. An app whose Space linked to it falls
    /// back to its own Space, by its own choice.
    Removed,
}

impl SpaceChange {
    /// The word on the bus.
    pub fn word(self) -> &'static str {
        match self {
            SpaceChange::Created => "created",
            SpaceChange::Renamed => "renamed",
            SpaceChange::Look => "look",
            SpaceChange::Removed => "removed",
        }
    }

    /// The change a word names.
    pub fn from_word(word: &str) -> Option<Self> {
        [
            SpaceChange::Created,
            SpaceChange::Renamed,
            SpaceChange::Look,
            SpaceChange::Removed,
        ]
        .into_iter()
        .find(|change| change.word() == word)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_what_the_person_typed_within_bounds() {
        let cases: &[(&str, &str, bool)] = &[
            ("plain", "Work", true),
            ("spaces and accents", "Été à Paris", true),
            ("emoji", "Home 🏡", true),
            ("blank", "   ", false),
            ("empty", "", false),
            ("newline", "a\nb", false),
            ("64 chars", &"é".repeat(64), true),
            ("65 chars", &"é".repeat(65), false),
        ];
        for (name, text, ok) in cases {
            assert_eq!(SpaceName::parse(text).is_ok(), *ok, "{name}");
        }
    }

    #[test]
    fn a_look_is_opaque_and_only_its_length_is_checked() {
        let json = r#"{"colour":"teal","symbol":"briefcase"}"#;
        assert_eq!(SpaceLook::parse(json).expect("look").as_str(), json);
        assert!(SpaceLook::parse("not json at all {").is_ok());
        assert!(SpaceLook::parse("").is_ok());
        assert!(SpaceLook::parse(&"x".repeat(1024)).is_ok());
        assert!(SpaceLook::parse(&"x".repeat(1025)).is_err());
        // Bytes, not characters: 512 two-byte characters fit, 513 do not.
        assert!(SpaceLook::parse(&"é".repeat(512)).is_ok());
        assert!(SpaceLook::parse(&"é".repeat(513)).is_err());
    }

    #[test]
    fn change_words_round_trip() {
        for change in [
            SpaceChange::Created,
            SpaceChange::Renamed,
            SpaceChange::Look,
            SpaceChange::Removed,
        ] {
            assert_eq!(SpaceChange::from_word(change.word()), Some(change));
            let json = serde_json::to_string(&change).expect("json");
            assert_eq!(json, format!("\"{}\"", change.word()));
        }
        assert_eq!(SpaceChange::from_word("moved"), None);
    }

    #[test]
    fn a_record_pins_its_json() {
        let record = DesktopSpaceRecord {
            id: DesktopSpace::parse("space-1").expect("id"),
            name: SpaceName::parse("Work").expect("name"),
            look: SpaceLook::parse(r#"{"c":1}"#).expect("look"),
            created: UnixSeconds(1_700_000_000),
        };
        let json = serde_json::to_string(&record).expect("json");
        assert_eq!(
            json,
            r#"{"id":"space-1","name":"Work","look":"{\"c\":1}","created":1700000000}"#
        );
        assert_eq!(
            serde_json::from_str::<DesktopSpaceRecord>(&json).expect("back"),
            record
        );
    }
}
