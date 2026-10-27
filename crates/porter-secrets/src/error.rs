//! porter-secrets' one error.

/// Why the store could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecretsError {
    /// Nothing is filed under that key.
    #[error("no such secret")]
    Missing,
    /// The store is locked and the user declined to unlock it.
    #[error("The keychain is locked and was not unlocked.")]
    Locked,
    /// No store is reachable (a headless session without a Secret Service). The text is what a
    /// person may read, so it names what to install, not the protocol.
    #[error(
        "No keychain is running. Install GNOME Keyring (the gnome-keyring package) or KDE Wallet, then sign out and back in."
    )]
    Unavailable,
    /// An item exists but does not decode as a credential.
    #[error("secret item unreadable")]
    Unreadable,
}
