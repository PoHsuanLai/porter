//! porter-secrets' one error.

/// Why the store could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    /// Nothing is filed under that key.
    #[error("no such secret")]
    Missing,
    /// The store is locked and the user declined to unlock it.
    #[error("The keychain is locked and was not unlocked.")]
    Locked,
    /// No store is reachable (a headless session without a Secret Service).
    ///
    /// The text is what a person may read, so it names what to install by family, not the
    /// protocol: GNOME Keyring is `gnome-keyring` everywhere; KDE Wallet is `kf6-kwallet` with
    /// `kwallet-pam` on Fedora and `kwallet` with `kwallet-pam` on Arch.
    #[error(
        "No keychain is running. Install GNOME Keyring (gnome-keyring) or KDE Wallet (kf6-kwallet and kwallet-pam on Fedora, kwallet and kwallet-pam on Arch), then sign out and sign in again."
    )]
    Unavailable,
    /// An item exists but does not decode as a credential.
    #[error("secret item unreadable")]
    Unreadable,
}
