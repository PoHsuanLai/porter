//! porter-secrets' one error.

/// Why the store could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    /// Nothing is filed under that key.
    #[error("no such secret")]
    Missing,
    /// The store is locked and the user declined to unlock it.
    #[error("secret store locked")]
    Locked,
    /// No store is reachable (a headless session without a Secret Service).
    #[error("secret store unavailable")]
    Unavailable,
    /// An item exists but does not decode as a credential.
    #[error("secret item unreadable")]
    Unreadable,
}
