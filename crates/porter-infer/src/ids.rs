//! Names and checked text on the inference wire. These mirror stoker's `model-provider` ids
//! (stoker has no porter dependency and porter-infer does not name its provider crate);
//! `inferd::bridge` converts between the two.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use porter_core::CoreError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Why wire text was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    /// The text is not JSON.
    #[error("not valid JSON: {0}")]
    NotJson(String),
    /// The text is not base64.
    #[error("not valid base64")]
    NotBase64,
}

/// The id a model gives one tool call, echoed by its result. Opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

/// The name of a function a model may call: 1 to 64 characters of `[A-Za-z0-9_.-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ToolName(String);

impl ToolName {
    /// The name written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let ok = (1..=64).contains(&text.len())
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "tool name",
                text: text.to_owned(),
            })
        }
    }

    /// The name's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ToolName {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<ToolName> for String {
    fn from(name: ToolName) -> String {
        name.0
    }
}

/// Text that parses as JSON, checked where it enters. Model output is untrusted, so tool
/// arguments cross the wire as `JsonText` and are read into a type by the code that owns it.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct JsonText(String);

impl JsonText {
    /// The text, if it is JSON.
    pub fn parse(text: &str) -> Result<Self, TextError> {
        serde_json::from_str::<serde::de::IgnoredAny>(text)
            .map_err(|e| TextError::NotJson(e.to_string()))?;
        Ok(Self(text.to_owned()))
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for JsonText {
    type Error = TextError;
    fn try_from(text: String) -> Result<Self, TextError> {
        Self::parse(&text)
    }
}

impl From<JsonText> for String {
    fn from(text: JsonText) -> String {
        text.0
    }
}

// Tool arguments can carry what the person typed: Debug shows the length only.
impl fmt::Debug for JsonText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsonText(<{} bytes>)", self.0.len())
    }
}

/// A JSON Schema, as text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonSchemaText(pub JsonText);

/// Bytes that travel in JSON as base64 text (a screenshot, an audio frame). Debug shows the
/// length only.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Base64Bytes(pub Vec<u8>);

impl fmt::Debug for Base64Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Base64Bytes(<{} bytes>)", self.0.len())
    }
}

impl Serialize for Base64Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD
            .decode(text)
            .map(Base64Bytes)
            .map_err(|_| serde::de::Error::custom(TextError::NotBase64))
    }
}

/// The index of a file descriptor received with SCM_RIGHTS on the `Open` fd, in the order sent
/// with the frame that names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttachIndex(pub u16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_follow_their_grammar() {
        let cases = [
            ("mail.thread.archive", true),
            ("a-b_c", true),
            ("", false),
            ("has space", false),
            (&"x".repeat(65), false),
        ];
        for (text, ok) in cases {
            assert_eq!(ToolName::parse(text).is_ok(), ok, "{text:?}");
        }
    }

    #[test]
    fn json_text_checks_and_hides() {
        assert!(JsonText::parse("{\"a\":1}").is_ok());
        assert!(JsonText::parse("{").is_err());
        let shown = format!("{:?}", JsonText::parse("\"secret\"").expect("json"));
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn base64_bytes_round_trip_as_text() {
        let bytes = Base64Bytes(vec![0, 1, 2, 250]);
        let json = serde_json::to_string(&bytes).expect("json");
        assert_eq!(json, "\"AAEC+g==\"");
        assert_eq!(
            serde_json::from_str::<Base64Bytes>(&json).expect("back"),
            bytes
        );
        assert!(serde_json::from_str::<Base64Bytes>("\"%%%\"").is_err());
        assert_eq!(format!("{bytes:?}"), "Base64Bytes(<4 bytes>)");
    }
}
