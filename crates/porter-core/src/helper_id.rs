//! The id of one of the person's helpers (a persistent named agent). It lives here, not in prov,
//! because both prov (`AgentRole::Helper`) and porter-infer (`Asker::Helper`) name it, and
//! porter-infer may depend only on porter-core and cua-action.

use porter_ids::IdError;
use serde::{Deserialize, Serialize};

/// One of the person's helpers. Opaque to porter (docket mints and owns it): 1 to 128 bytes, no
/// control characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HelperId(String);

impl HelperId {
    /// The id written as `text`, or why it is not one.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if !text.is_empty() && text.len() <= 128 && !text.chars().any(char::is_control) {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError::malformed("helper id", text))
        }
    }
}

porter_ids::string_newtype!(HelperId, IdError);
