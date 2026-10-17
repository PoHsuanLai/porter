//! The id of one of the person's helpers (a persistent named agent). It lives here, not in prov,
//! because both prov (`AgentRole::Helper`) and porter-infer (`Asker::Helper`) name it, and
//! porter-infer may depend only on porter-core and cua-action.

use crate::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// One of the person's helpers. Opaque to porter (docket mints and owns it): 1 to 128 bytes, no
/// control characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HelperId(String);

impl HelperId {
    /// The id written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        if !text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control) {
            Ok(Self(text.to_owned()))
        } else {
            Err(CoreError::MalformedId {
                what: "helper id",
                text: text.to_owned(),
            })
        }
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for HelperId {
    type Error = CoreError;
    fn try_from(text: String) -> Result<Self, CoreError> {
        Self::parse(&text)
    }
}

impl From<HelperId> for String {
    fn from(id: HelperId) -> String {
        id.0
    }
}

impl fmt::Display for HelperId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
