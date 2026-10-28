//! A provider's mark as data: the letter and the colour a provider file gives its round mark, so
//! the UI can draw a provider it has no named mark for. The order of use in the UI is the face,
//! then the named variant (`ProviderRow::mark`), then the neutral "@". The UI picks the ink
//! colour for contrast itself, so a face carries only the fill.
//!
//! Both parts are checked where they are made (`parse`) and where they are read (serde goes
//! through `parse`), so a value that exists is a value a UI can draw.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Why a mark face part was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MarkFaceError {
    /// The letter is not one or two characters, or has whitespace or a control character.
    #[error("mark letter {0:?} is not one or two visible characters")]
    Letter(String),
    /// The colour is not `#RRGGBB`.
    #[error("mark colour {0:?} is not #RRGGBB")]
    Colour(String),
}

/// One or two characters (not bytes), none of them whitespace or control.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MarkLetter(String);

impl MarkLetter {
    /// The letter written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, MarkFaceError> {
        let count = text.chars().count();
        let visible = text.chars().all(|c| !c.is_whitespace() && !c.is_control());
        match (1..=2).contains(&count) && visible {
            true => Ok(Self(text.to_owned())),
            false => Err(MarkFaceError::Letter(text.to_owned())),
        }
    }

    /// The letter's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MarkLetter {
    type Error = MarkFaceError;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<MarkLetter> for String {
    fn from(letter: MarkLetter) -> String {
        letter.0
    }
}

impl fmt::Display for MarkLetter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A colour written `#RRGGBB`: `#` and six hex digits, stored upper-case. No alpha, no short
/// form, no names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MarkColour(String);

impl MarkColour {
    /// The colour written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, MarkFaceError> {
        let digits = text.strip_prefix('#').unwrap_or("");
        match digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            true => Ok(Self(text.to_ascii_uppercase())),
            false => Err(MarkFaceError::Colour(text.to_owned())),
        }
    }

    /// The colour as `#RRGGBB`, upper-case.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MarkColour {
    type Error = MarkFaceError;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<MarkColour> for String {
    fn from(colour: MarkColour) -> String {
        colour.0
    }
}

impl fmt::Display for MarkColour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The face a provider file gives its mark: a letter on a colour.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarkFace {
    /// The letter drawn on the mark.
    pub letter: MarkLetter,
    /// The mark's fill.
    pub colour: MarkColour,
}

#[cfg(test)]
mod tests;
