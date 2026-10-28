//! The one error of this crate: text that should have been a value of ours is not one.

/// Text that should have been an identifier or URL of the named kind is not one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed {what}: {text:?}")]
pub struct IdError {
    what: &'static str,
    text: String,
}

impl IdError {
    /// The refusal of `text` as a `what` (`"account id"`, `"web url"`).
    pub fn malformed(what: &'static str, text: &str) -> Self {
        Self {
            what,
            text: text.to_owned(),
        }
    }

    /// The kind of value that was expected.
    pub fn what(&self) -> &'static str {
        self.what
    }

    /// The text refused.
    pub fn text(&self) -> &str {
        &self.text
    }
}
