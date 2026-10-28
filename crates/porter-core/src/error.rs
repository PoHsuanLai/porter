//! porter-core's one error: values from outside that are not values of ours.

/// Why a value from outside (a file, the bus, a socket) was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// Text that should have been an identifier of the named kind is not one.
    #[error("malformed {what}: {text:?}")]
    MalformedId {
        /// The kind of identifier (`"account id"`, `"app name"`).
        what: &'static str,
        /// The text refused.
        text: String,
    },
    /// A value that arrived from outside (a bus argument) is not a value of ours.
    #[error("malformed value: {0}")]
    MalformedValue(String),
}

impl From<porter_ids::IdError> for CoreError {
    fn from(error: porter_ids::IdError) -> Self {
        CoreError::MalformedId {
            what: error.what(),
            text: error.text().to_owned(),
        }
    }
}
