//! Why Tailscale could not be asked, in the words a person reads.

/// Why a question to Tailscale got no answer. Each variant's text is a plain sentence for the
/// person; the program names of porter never reach them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TailscaleError {
    /// There is no Tailscale on this computer.
    #[error("Tailscale isn't installed on this computer.")]
    NotInstalled,
    /// It is installed, but its service is not answering.
    #[error("Tailscale isn't running on this computer.")]
    NotRunning,
    /// Its socket is there, but it will not talk to this program (it is not open to every user
    /// and this person is not its operator).
    #[error("This computer's Tailscale doesn't let porter ask it yet.")]
    Refused,
    /// It answers, but nobody is signed in to it.
    #[error("Tailscale is signed out.")]
    SignedOut,
    /// There is no computer on the network by that address.
    #[error("Tailscale does not know that computer.")]
    NoSuchPeer,
    /// It answered something that is not what it should say (a version this does not know).
    #[error("Tailscale gave an answer porter could not read.")]
    Malformed,
    /// It did not answer in time.
    #[error("Tailscale did not answer in time.")]
    TimedOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sentence_is_plain_and_ends_with_a_full_stop() {
        let all = [
            TailscaleError::NotInstalled,
            TailscaleError::NotRunning,
            TailscaleError::Refused,
            TailscaleError::SignedOut,
            TailscaleError::NoSuchPeer,
            TailscaleError::Malformed,
            TailscaleError::TimedOut,
        ];
        for error in all {
            let text = error.to_string();
            assert!(text.ends_with('.'), "{text}");
            for jargon in ["LocalAPI", "socket", "JSON", "HTTP", "daemon", "tailscaled"] {
                assert!(!text.contains(jargon), "{text}");
            }
        }
        assert_eq!(
            TailscaleError::NotRunning.to_string(),
            "Tailscale isn't running on this computer."
        );
        assert_eq!(
            TailscaleError::NotInstalled.to_string(),
            "Tailscale isn't installed on this computer."
        );
        assert_eq!(
            TailscaleError::SignedOut.to_string(),
            "Tailscale is signed out."
        );
        assert_eq!(
            TailscaleError::Refused.to_string(),
            "This computer's Tailscale doesn't let porter ask it yet."
        );
    }
}
